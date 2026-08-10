//! Manages the pki-bundle used to sign r-boot and the kernels/initrds it
//! boots for UEFI secure boot: an RSA-2048 keypair and a self-signed
//! certificate, stored as `db.key` (root-only, never leaves the host) and
//! `db.pem` (the public certificate enrolled into the firmware's `db`).
//! Signing itself lands in a later change; this only manages the bundle.
//!
//! Key generation and signing go through the pure-Rust `rsa` crate rather
//! than rcgen's `ring`/`aws_lc_rs` backends, which are C libraries requiring
//! a C toolchain and cmake at build time; this keeps `cargo build --offline`
//! (as used by `nix/package.nix`) free of new native build dependencies.

use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use rand::rngs::OsRng;
use rand::RngCore;
use rcgen::{
    CertificateParams, DistinguishedName, DnType, IsCa, KeyUsagePurpose, PKCS_RSA_SHA256,
    PublicKeyData, SerialNumber, SignatureAlgorithm, SigningKey as RcgenSigningKey,
};
use rsa::pkcs1::EncodeRsaPublicKey;
use rsa::pkcs1v15;
use rsa::pkcs8::EncodePrivateKey;
use rsa::RsaPrivateKey;
use sha2::{Digest, Sha256};
use signature::Signer;
use time::{Duration, OffsetDateTime};
use x509_parser::pem::parse_x509_pem;
use x509_parser::prelude::{FromDer, X509Certificate};

const PKI_DIR: &str = "/var/lib/r-boot/pki";
const KEY_FILE: &str = "db.key";
const CERT_FILE: &str = "db.pem";
const VALIDITY_DAYS: i64 = 3650;
const RSA_KEY_BITS: usize = 2048;

/// Adapts an `rsa` crate keypair to rcgen's `SigningKey`/`PublicKeyData`
/// traits, so rcgen never has to generate or touch the private key itself.
struct RsaSigningKey {
    signing_key: pkcs1v15::SigningKey<Sha256>,
    public_key_der: Vec<u8>,
}

impl RsaSigningKey {
    fn new(private_key: RsaPrivateKey) -> Result<Self, Box<dyn Error>> {
        let public_key_der = private_key.to_public_key().to_pkcs1_der()?.into_vec();
        Ok(Self {
            signing_key: pkcs1v15::SigningKey::<Sha256>::new(private_key),
            public_key_der,
        })
    }
}

impl PublicKeyData for RsaSigningKey {
    fn der_bytes(&self) -> &[u8] {
        &self.public_key_der
    }

    fn algorithm(&self) -> &'static SignatureAlgorithm {
        &PKCS_RSA_SHA256
    }
}

impl RcgenSigningKey for RsaSigningKey {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
        use signature::SignatureEncoding;
        Ok(self.signing_key.sign(msg).to_vec())
    }
}

pub fn key_path() -> std::path::PathBuf {
    Path::new(PKI_DIR).join(KEY_FILE)
}

pub fn cert_path() -> std::path::PathBuf {
    Path::new(PKI_DIR).join(CERT_FILE)
}

/// Reads the pki-bundle's certificate and returns its raw DER bytes.
pub fn cert_der() -> Result<Vec<u8>, Box<dyn Error>> {
    let cert_path = cert_path();
    let pem_bytes =
        fs::read(&cert_path).map_err(|e| format!("cannot read {}: {e}", cert_path.display()))?;
    let (_, pem) = parse_x509_pem(&pem_bytes)
        .map_err(|e| format!("cannot parse {}: {e}", cert_path.display()))?;
    Ok(pem.contents)
}

pub fn create(force: bool) -> Result<(), Box<dyn Error>> {
    if !running_as_root() {
        return Err(
            "sign-key create must be run as root (db.key must be root-owned and root-only readable)"
                .into(),
        );
    }

    let dir = Path::new(PKI_DIR);
    let key_path = dir.join(KEY_FILE);
    let cert_path = dir.join(CERT_FILE);

    if !force && (key_path.exists() || cert_path.exists()) {
        return Err(format!(
            "pki-bundle already exists at {} (use --force to overwrite)",
            dir.display()
        )
        .into());
    }

    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;

    println!("generating RSA-{RSA_KEY_BITS} keypair (this can take a few seconds)...");
    let private_key = RsaPrivateKey::new(&mut OsRng, RSA_KEY_BITS)?;
    let key_pem = private_key.to_pkcs8_pem(Default::default())?;
    let signing_key = RsaSigningKey::new(private_key)?;

    let mut params = CertificateParams::new(Vec::<String>::new())?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "r-boot secure boot db");
    params.distinguished_name = dn;
    let now = OffsetDateTime::now_utc();
    params.not_before = now;
    params.not_after = now + Duration::days(VALIDITY_DAYS);
    params.is_ca = IsCa::NoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.serial_number = Some(random_serial_number());

    let cert = params.self_signed(&signing_key)?;

    write_file(&key_path, key_pem.as_bytes(), 0o600)?;
    write_file(&cert_path, cert.pem().as_bytes(), 0o644)?;

    println!("generated pki-bundle in {}:", dir.display());
    println!("  {}  (private key, root-only)", key_path.display());
    println!("  {}  (public certificate)", cert_path.display());
    Ok(())
}

pub fn show() -> Result<(), Box<dyn Error>> {
    let dir = Path::new(PKI_DIR);
    let key_path = dir.join(KEY_FILE);
    let cert_path = dir.join(CERT_FILE);

    if !cert_path.exists() {
        println!("no pki-bundle found at {}", dir.display());
        println!("run `r-boot-cli sign-key create` (as root) to generate one");
        return Ok(());
    }

    let pem_bytes = fs::read(&cert_path)
        .map_err(|e| format!("cannot read {}: {e}", cert_path.display()))?;
    let (_, pem) = parse_x509_pem(&pem_bytes)
        .map_err(|e| format!("cannot parse {}: {e}", cert_path.display()))?;
    let (_, cert) = X509Certificate::from_der(&pem.contents)
        .map_err(|e| format!("cannot parse {}: {e}", cert_path.display()))?;

    let fingerprint: String = Sha256::digest(&pem.contents)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let validity = cert.validity();

    println!("pki-bundle: {}", dir.display());
    println!("  certificate: {}", cert_path.display());
    println!(
        "  private key: {} ({})",
        key_path.display(),
        if key_path.exists() {
            "present"
        } else {
            "MISSING"
        }
    );
    println!("  subject:     {}", cert.subject());
    println!("  not before:  {}", validity.not_before);
    println!("  not after:   {}", validity.not_after);
    println!(
        "  status:      {}",
        if validity.is_valid() {
            "valid"
        } else {
            "expired or not yet valid"
        }
    );
    println!("  sha256:      {fingerprint}");

    Ok(())
}

pub fn remove(force: bool) -> Result<(), Box<dyn Error>> {
    let dir = Path::new(PKI_DIR);
    let key_path = dir.join(KEY_FILE);
    let cert_path = dir.join(CERT_FILE);

    if !key_path.exists() && !cert_path.exists() {
        println!("no pki-bundle found at {}", dir.display());
        return Ok(());
    }

    if !force {
        return Err(format!(
            "refusing to remove the pki-bundle at {} without --force: this permanently discards \
             the secure boot signing key; anything signed with it stops being trustable once the \
             matching certificate is dropped from the firmware's db",
            dir.display()
        )
        .into());
    }

    for path in [&key_path, &cert_path] {
        if path.exists() {
            fs::remove_file(path)?;
            println!("removed {}", path.display());
        }
    }
    Ok(())
}

fn write_file(path: &Path, contents: &[u8], mode: u32) -> Result<(), Box<dyn Error>> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    file.write_all(contents)?;
    // OpenOptionsExt::mode is masked by umask on creation; pin the exact
    // bits so a permissive umask can't leave db.key group/other readable.
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn running_as_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// A random 20-byte X.509 serial number. Without the `crypto` feature (kept
/// off to stay off rcgen's C `ring`/`aws_lc_rs` backends), rcgen doesn't
/// generate one itself. The top bit of the first byte is cleared so the
/// DER INTEGER encoding stays unambiguously positive.
fn random_serial_number() -> SerialNumber {
    let mut bytes = [0u8; 20];
    OsRng.fill_bytes(&mut bytes);
    bytes[0] &= 0x7f;
    SerialNumber::from(bytes.to_vec())
}
