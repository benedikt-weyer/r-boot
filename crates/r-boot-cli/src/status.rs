//! `r-boot-cli status`: whether the pki-bundle exists, whether r-boot is
//! installed on the ESP, and whether the installed binary is
//! Authenticode-signed (and with which certificate).

use std::error::Error;
use std::path::Path;

use cms::cert::CertificateChoices;
use cms::content_info::ContentInfo;
use cms::signed_data::SignedData;
use der::{Decode, Encode};
use sha2::{Digest, Sha256};

use crate::sign_key;

/// `WIN_CERTIFICATE.wCertificateType` for an Authenticode PKCS#7 SignedData
/// blob (as opposed to the raw-X.509 `WIN_CERT_TYPE_EFI_GUID` r-boot's own
/// `secureboot` enrollment code uses for db/KEK/PK, a different structure).
const WIN_CERT_TYPE_PKCS_SIGNED_DATA: u16 = 0x0002;

pub fn show(esp: &Path) -> Result<(), Box<dyn Error>> {
    let key_path = sign_key::key_path();
    let cert_path = sign_key::cert_path();
    let cert_der = sign_key::cert_der().ok();

    println!("pki-bundle:");
    println!(
        "  private key: {} ({})",
        key_path.display(),
        if key_path.exists() { "present" } else { "missing" }
    );
    println!(
        "  certificate: {} ({})",
        cert_path.display(),
        if cert_der.is_some() { "present" } else { "missing" }
    );
    println!();

    let boot_efi = esp.join("EFI/BOOT/BOOTX64.EFI");
    let Ok(bytes) = std::fs::read(&boot_efi) else {
        println!("bootloader: not installed at {}", boot_efi.display());
        return Ok(());
    };
    println!("bootloader: installed at {}", boot_efi.display());

    let signer_cert_der = extract_pkcs7(&bytes).and_then(|pkcs7| signer_cert(&pkcs7));
    println!("  signed: {}", if signer_cert_der.is_some() { "yes" } else { "no" });

    if let Some(signer_cert_der) = &signer_cert_der {
        match &cert_der {
            Some(cert_der) => {
                let matches = fingerprint(signer_cert_der) == fingerprint(cert_der);
                println!(
                    "  signed with {} cert: {}",
                    cert_path.display(),
                    if matches { "yes" } else { "no (different certificate)" }
                );
            }
            None => println!(
                "  signed with {} cert: unknown (no certificate there to compare against)",
                cert_path.display()
            ),
        }
    }

    Ok(())
}

fn fingerprint(der: &[u8]) -> [u8; 32] {
    Sha256::digest(der).into()
}

/// The signer certificate embedded in a PE file's Authenticode PKCS#7
/// SignedData, if any is present and well-formed.
fn signer_cert(pkcs7_der: &[u8]) -> Option<Vec<u8>> {
    let content_info = ContentInfo::from_der(pkcs7_der).ok()?;
    let signed_data_der = content_info.content.to_der().ok()?;
    let signed_data = SignedData::from_der(&signed_data_der).ok()?;
    let certificates = signed_data.certificates?;
    certificates.0.iter().find_map(|choice| match choice {
        CertificateChoices::Certificate(certificate) => certificate.to_der().ok(),
        CertificateChoices::Other(_) => None,
    })
}

/// The raw bytes of a PE/COFF file's Authenticode certificate table entry
/// (the PKCS#7 SignedData `ContentInfo`, DER-encoded), if present. Reads
/// just enough of the DOS/PE/Optional headers to find it; never panics on
/// malformed input, only returns `None`.
fn extract_pkcs7(bytes: &[u8]) -> Option<Vec<u8>> {
    let read_u16 = |offset: usize| -> Option<u16> {
        Some(u16::from_le_bytes(bytes.get(offset..offset + 2)?.try_into().ok()?))
    };
    let read_u32 = |offset: usize| -> Option<u32> {
        Some(u32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?))
    };

    if bytes.get(0..2)? != b"MZ" {
        return None;
    }
    let pe_header = read_u32(0x3C)? as usize;
    if bytes.get(pe_header..pe_header + 4)? != b"PE\0\0" {
        return None;
    }

    // COFF file header, right after the PE signature: Machine(2),
    // NumberOfSections(2), TimeDateStamp(4), PointerToSymbolTable(4),
    // NumberOfSymbols(4), SizeOfOptionalHeader(2), Characteristics(2).
    let coff_header = pe_header + 4;
    let size_of_optional_header = read_u16(coff_header + 16)? as usize;
    let optional_header = coff_header + 20;

    // The Data Directories array sits at a fixed offset into the Optional
    // Header, which differs between PE32 and PE32+ (the only kind UEFI
    // binaries use, but both are handled here for completeness).
    let magic = read_u16(optional_header)?;
    let data_directories = match magic {
        0x10b => optional_header + 96,
        0x20b => optional_header + 112,
        _ => return None,
    };

    // IMAGE_DIRECTORY_ENTRY_SECURITY (the Authenticode certificate table)
    // is data directory index 4; each entry is an 8-byte (offset, size) pair.
    const SECURITY_DIRECTORY_INDEX: usize = 4;
    let entry = data_directories + SECURITY_DIRECTORY_INDEX * 8;
    if entry + 8 > optional_header + size_of_optional_header {
        return None;
    }
    let cert_table_offset = read_u32(entry)? as usize;
    let cert_table_size = read_u32(entry + 4)? as usize;
    if cert_table_size == 0 {
        return None;
    }

    // WIN_CERTIFICATE: dwLength(4), wRevision(2), wCertificateType(2),
    // followed by dwLength - 8 bytes of certificate data. The certificate
    // table as a whole is 8-byte aligned, so cert_table_size can include a
    // trailing padding byte beyond dwLength that isn't part of the DER
    // payload — bound the slice by dwLength, not cert_table_size.
    let cert_entry = bytes.get(cert_table_offset..cert_table_offset + cert_table_size)?;
    if cert_entry.len() < 8 {
        return None;
    }
    let dw_length = u32::from_le_bytes(cert_entry[0..4].try_into().ok()?) as usize;
    let cert_type = u16::from_le_bytes(cert_entry[6..8].try_into().ok()?);
    if cert_type != WIN_CERT_TYPE_PKCS_SIGNED_DATA || dw_length < 8 {
        return None;
    }
    Some(cert_entry.get(8..dw_length)?.to_vec())
}
