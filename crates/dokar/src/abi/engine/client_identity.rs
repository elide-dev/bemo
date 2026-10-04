use super::{certificates, lock};
use rustls::client::ResolvesClientCert;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::PrivateKeyDer;
use rustls::sign::CertifiedKey;
use std::sync::{Arc, Mutex};

pub(super) type Selected = Arc<Mutex<Option<Arc<CertifiedKey>>>>;

#[derive(Debug)]
pub(super) struct Identity {
  key: Arc<CertifiedKey>,
  issuers: Vec<Vec<u8>>,
}

impl Identity {
  pub(super) fn pem(input: &[u8], provider: &CryptoProvider) -> Option<Self> {
    use rustls::pki_types::pem::PemObject;
    Some(Self {
      key: Arc::new(
        CertifiedKey::from_der(
          certificates(input, false)?,
          PrivateKeyDer::from_pem_slice(input).ok()?,
          provider,
        )
        .ok()?,
      ),
      issuers: Vec::new(),
    })
  }
}

fn take<'a>(input: &mut &'a [u8], length: usize) -> Option<&'a [u8]> {
  let (value, rest) = input.split_at_checked(length)?;
  *input = rest;
  Some(value)
}

fn count(input: &mut &[u8]) -> Option<usize> {
  Some(usize::from(u16::from_be_bytes(take(input, 2)?.try_into().ok()?)))
}

fn blob<'a>(input: &mut &'a [u8]) -> Option<&'a [u8]> {
  let length = usize::try_from(u32::from_be_bytes(take(input, 4)?.try_into().ok()?)).ok()?;
  take(input, length)
}

/// Version 1: identity count, then DER chain/key blobs and issuer-DN vectors, all big endian.
pub(super) fn parse(mut input: &[u8], provider: &CryptoProvider) -> Option<Vec<Identity>> {
  if take(&mut input, 1)? != [1] {
    return None;
  }
  let entries = count(&mut input)?;
  if !(1..=64).contains(&entries) {
    return None;
  }
  let mut identities = Vec::with_capacity(entries);
  for _ in 0..entries {
    let chain = certificates(blob(&mut input)?, true)?;
    let key = PrivateKeyDer::try_from(blob(&mut input)?).ok()?.clone_key();
    let key = Arc::new(CertifiedKey::from_der(chain, key, provider).ok()?);
    let names = count(&mut input)?;
    if !(1..=64).contains(&names) {
      return None;
    }
    let mut issuers = Vec::with_capacity(names);
    for _ in 0..names {
      let length = count(&mut input)?;
      if length == 0 {
        return None;
      }
      issuers.push(take(&mut input, length)?.to_vec());
    }
    identities.push(Identity { key, issuers });
  }
  input.is_empty().then_some(identities)
}

#[derive(Debug)]
pub(super) struct Resolver {
  pub identities: Arc<Vec<Identity>>,
  pub selected: Selected,
}

impl ResolvesClientCert for Resolver {
  fn resolve(&self, roots: &[&[u8]], schemes: &[rustls::SignatureScheme]) -> Option<Arc<CertifiedKey>> {
    let selected = self
      .identities
      .iter()
      .find(|identity| {
        (roots.is_empty()
          || identity.issuers.is_empty()
          || roots
            .iter()
            .any(|root| identity.issuers.iter().any(|issuer| root == &issuer.as_slice())))
          && identity.key.key.choose_scheme(schemes).is_some()
      })
      .map(|identity| identity.key.clone());
    *lock(&self.selected) = selected.clone();
    selected
  }

  fn has_certs(&self) -> bool {
    !self.identities.is_empty()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  #[cfg_attr(miri, ignore = "AWS-LC key loading")]
  fn selection_requires_an_offered_signature_scheme() {
    let input = [
      include_bytes!("../../../tests/fixtures/client-cert.pem").as_slice(),
      b"\n",
      include_bytes!("../../../tests/fixtures/client-key.pem").as_slice(),
    ]
    .concat();
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let resolver = Resolver {
      identities: Arc::new(vec![Identity::pem(&input, &provider).unwrap()]),
      selected: Selected::default(),
    };
    assert!(
      resolver
        .resolve(&[], &[rustls::SignatureScheme::ECDSA_NISTP256_SHA256])
        .is_none()
    );
    assert!(lock(&resolver.selected).is_none());
    assert!(
      resolver
        .resolve(&[], &[rustls::SignatureScheme::RSA_PSS_SHA256])
        .is_some()
    );
    assert!(lock(&resolver.selected).is_some());
    assert!(resolver.resolve(&[], &[]).is_none());
    assert!(lock(&resolver.selected).is_none());
  }
}
