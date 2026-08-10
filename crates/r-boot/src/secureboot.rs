//! Enrolls a self-issued certificate as UEFI Secure Boot's PK, KEK and db,
//! reachable from the boot menu via the `e` key.
//!
//! This only implements the *Setup Mode* bootstrapping path: PK/KEK/db are
//! UEFI "time-based authenticated" variables, and once a Platform Key is
//! set, firmware requires further updates to be signed with a real PKCS#7
//! signature. But while `SetupMode == 1` (no PK enrolled yet), firmware
//! skips that signature check and accepts an update with an empty one — the
//! same mechanism `KeyTool.efi` and `sbctl`'s "enroll keys" use to bootstrap
//! a first, self-issued key from an unsigned environment. The payload still
//! has to be wrapped in the `EFI_VARIABLE_AUTHENTICATION_2` shape (a
//! timestamp plus a `WIN_CERTIFICATE_UEFI_GUID` header), just with a
//! zero-length signature.

use alloc::vec::Vec;

use uefi::boot;
use uefi::fs::{FileSystem, Path};
use uefi::proto::console::text::Key;
use uefi::runtime::{self, VariableAttributes, VariableVendor};
use uefi::{CStr16, Guid, cstr16, guid, system};

/// Where `nix/module.nix`'s signing step places the DER-encoded certificate
/// alongside the signed BOOTX64.EFI, when `sign-bootloader` is enabled.
const CERT_PATH: &CStr16 = cstr16!("\\EFI\\BOOT\\db.der");

const CERT_X509_GUID: Guid = guid!("a5c059a1-94e4-4aa7-87b5-ab155c2bf072");
const CERT_TYPE_PKCS7_GUID: Guid = guid!("4aafd29d-68df-49ee-8aa9-347d375665a7");
const WIN_CERT_TYPE_EFI_GUID: u16 = 0x0EF1;
const WIN_CERT_REVISION: u16 = 0x0200;

/// Owner GUID stamped on the signature list entries r-boot adds. Arbitrary
/// per spec (it just identifies who added a given entry); fixed here so
/// repeated enrollments are recognizable if the db is inspected later.
const SIGNATURE_OWNER: Guid = guid!("2916e9be-32b3-45f0-8af6-25ab8643fdc4");

const AUTH_ATTRIBUTES: VariableAttributes = VariableAttributes::NON_VOLATILE
    .union(VariableAttributes::BOOTSERVICE_ACCESS)
    .union(VariableAttributes::RUNTIME_ACCESS)
    .union(VariableAttributes::TIME_BASED_AUTHENTICATED_WRITE_ACCESS);

/// Never propagates an error out of the menu: every expected failure (not
/// in Setup Mode, no certificate, user cancelled) is reported inline and
/// returns control to the caller so a botched attempt doesn't take down the
/// whole boot menu.
pub fn enroll(fs: &mut FileSystem) {
    if !confirm() {
        return;
    }
    if !is_setup_mode() {
        notify(&[
            "Secure boot keys are already enrolled (firmware is not in Setup Mode).",
            "Clear the existing Platform Key from firmware setup to re-enroll.",
        ]);
        return;
    }
    let cert = match fs.read(Path::new(CERT_PATH)) {
        Ok(bytes) => bytes,
        Err(_) => {
            notify(&[
                "No certificate found at \\EFI\\BOOT\\db.der.",
                "Rebuild with boot.loader.r-boot.sign-bootloader enabled first.",
            ]);
            return;
        }
    };
    let Some(timestamp) = timestamp_bytes() else {
        notify(&["Cannot read the firmware clock; aborting enrollment."]);
        return;
    };
    let payload = authenticated_payload(&timestamp, &build_signature_list(&cert));

    if let Err(message) = enroll_all(&payload) {
        notify(&["Enrollment failed:", message]);
        return;
    }
    notify(&[
        "Secure boot keys enrolled.",
        "Reboot for the new Platform Key to take effect.",
    ]);
}

/// Writes db and KEK, then writes PK last — writing PK is what exits Setup
/// Mode, so it has to be the final step. Also tries to flip the
/// (EDK2-specific, not baseline-spec) `SecureBootEnable` toggle on before
/// that, best-effort: firmware that doesn't support it as a plain write
/// pre-PK rejects it, but firmware enforces secure boot once a valid PK
/// exists regardless, so that failure alone isn't fatal to enrollment.
fn enroll_all(payload: &[u8]) -> Result<(), &'static str> {
    set_auth_variable(cstr16!("db"), VariableVendor::IMAGE_SECURITY_DATABASE, payload)
        .map_err(|_| "firmware rejected the db update")?;
    set_auth_variable(cstr16!("KEK"), VariableVendor::GLOBAL_VARIABLE, payload)
        .map_err(|_| "firmware rejected the KEK update")?;
    let _ = set_secure_boot_enable();
    set_auth_variable(cstr16!("PK"), VariableVendor::GLOBAL_VARIABLE, payload)
        .map_err(|_| "firmware rejected the PK update")?;
    Ok(())
}

fn set_auth_variable(name: &CStr16, vendor: VariableVendor, payload: &[u8]) -> uefi::Result {
    runtime::set_variable(name, &vendor, AUTH_ATTRIBUTES, payload)
}

fn set_secure_boot_enable() -> Result<(), &'static str> {
    let attributes = VariableAttributes::NON_VOLATILE
        | VariableAttributes::BOOTSERVICE_ACCESS
        | VariableAttributes::RUNTIME_ACCESS;
    runtime::set_variable(
        cstr16!("SecureBootEnable"),
        &VariableVendor::GLOBAL_VARIABLE,
        attributes,
        &[1u8],
    )
    .map_err(|_| "firmware rejected enabling secure boot")
}

fn is_setup_mode() -> bool {
    let mut buffer = [0u8; 1];
    match runtime::get_variable(cstr16!("SetupMode"), &VariableVendor::GLOBAL_VARIABLE, &mut buffer) {
        Ok((data, _)) => data.first() == Some(&1),
        Err(_) => false,
    }
}

/// The `EFI_TIME` timestamp an `EFI_VARIABLE_AUTHENTICATION_2` payload
/// starts with.
fn timestamp_bytes() -> Option<[u8; 16]> {
    // `Time` is `#[repr(transparent)]` over `uefi_raw::time::Time`, a
    // `#[repr(C)]` 16-byte struct with every field (including its two
    // padding bytes) always explicitly written — an exact match for
    // `EFI_TIME`'s wire format, so this is a safe reinterpretation.
    const _: () = assert!(core::mem::size_of::<runtime::Time>() == 16);
    let time = runtime::get_time().ok()?;
    Some(unsafe { core::mem::transmute::<runtime::Time, [u8; 16]>(time) })
}

/// Wraps `signature_list` in an `EFI_VARIABLE_AUTHENTICATION_2` header with
/// an empty PKCS#7 blob — the shape Setup Mode accepts without actually
/// checking a signature.
fn authenticated_payload(timestamp: &[u8; 16], signature_list: &[u8]) -> Vec<u8> {
    let cert_type = CERT_TYPE_PKCS7_GUID.to_bytes();
    // WIN_CERTIFICATE_UEFI_GUID's dwLength covers its own header (the
    // WIN_CERTIFICATE Hdr: dwLength + wRevision + wCertificateType, 8
    // bytes) plus CertType (16 bytes) plus CertData, which is empty here.
    let hdr_length: u32 = 4 + 2 + 2 + 16;
    let mut payload = Vec::with_capacity(16 + hdr_length as usize + signature_list.len());
    payload.extend_from_slice(timestamp);
    payload.extend_from_slice(&hdr_length.to_le_bytes());
    payload.extend_from_slice(&WIN_CERT_REVISION.to_le_bytes());
    payload.extend_from_slice(&WIN_CERT_TYPE_EFI_GUID.to_le_bytes());
    payload.extend_from_slice(&cert_type);
    payload.extend_from_slice(signature_list);
    payload
}

/// Wraps `cert_der` in a single-entry `EFI_SIGNATURE_LIST` of X.509
/// certificates, the format PK/KEK/db entries use.
fn build_signature_list(cert_der: &[u8]) -> Vec<u8> {
    let signature_size: u32 = 16 + cert_der.len() as u32; // owner GUID + cert
    let list_size: u32 = 16 + 4 + 4 + 4 + signature_size; // type GUID + 3 u32 fields + entry
    let mut out = Vec::with_capacity(list_size as usize);
    out.extend_from_slice(&CERT_X509_GUID.to_bytes());
    out.extend_from_slice(&list_size.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // SignatureHeaderSize
    out.extend_from_slice(&signature_size.to_le_bytes());
    out.extend_from_slice(&SIGNATURE_OWNER.to_bytes());
    out.extend_from_slice(cert_der);
    out
}

fn confirm() -> bool {
    system::with_stdout(|output| {
        let _ = output.clear();
    });
    uefi::println!("Enroll secure boot keys");
    uefi::println!();
    uefi::println!("This installs \\EFI\\BOOT\\db.der as the platform's Secure Boot");
    uefi::println!("Platform Key, Key Exchange Key, and signature database. It only");
    uefi::println!("works once, before any Platform Key is enrolled, and can't be");
    uefi::println!("undone from here afterwards.");
    uefi::println!();
    uefi::println!("Press y to confirm, any other key cancels.");
    matches!(read_key(), Some(Key::Printable(character)) if character == 'y' || character == 'Y')
}

fn notify(lines: &[&str]) {
    system::with_stdout(|output| {
        let _ = output.clear();
    });
    for line in lines {
        uefi::println!("{line}");
    }
    uefi::println!();
    uefi::println!("Press any key to continue.");
    let _ = read_key();
}

fn read_key() -> Option<Key> {
    let key_event = system::with_stdin(|input| input.wait_for_key_event()).ok()?;
    let mut events = unsafe { [key_event.unsafe_clone()] };
    boot::wait_for_event(&mut events).ok()?;
    system::with_stdin(|input| input.read_key()).ok().flatten()
}
