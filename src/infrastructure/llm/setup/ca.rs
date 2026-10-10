use std::{io::Read, path::Path};

use rustls::pki_types::{CertificateDer, pem::PemObject};

use super::SetupError;

const MAX_CA_BYTES: u64 = 1_048_576;

pub(super) fn read(path: &Path) -> Result<Vec<CertificateDer<'static>>, SetupError> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(MAX_CA_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| SetupError::CaFile)?;
    if bytes.len() as u64 > MAX_CA_BYTES {
        return Err(SetupError::CaFile);
    }
    let pem: Result<Vec<_>, _> = CertificateDer::pem_slice_iter(&bytes).collect();
    match pem {
        Ok(certs) if !certs.is_empty() => Ok(certs),
        // A DER certificate starts with a SEQUENCE tag.
        Ok(_) if bytes.first() == Some(&0x30) => Ok(vec![CertificateDer::from(bytes)]),
        _ => Err(SetupError::CaFile),
    }
}
