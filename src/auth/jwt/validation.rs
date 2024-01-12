use jwtk::Claims;

use crate::blueprint;

pub fn validate_iss(options: &blueprint::JwtProvider, claims: &Claims<()>) -> bool {
  options
    .issuer
    .as_ref()
    .map(|issuer| claims.iss.as_ref().map(|iss| iss == issuer).unwrap_or(false))
    .unwrap_or(true)
}

pub fn validate_aud(options: &blueprint::JwtProvider, claims: &Claims<()>) -> bool {
  let audiences = &options.audiences;

  if audiences.is_empty() {
    true
  } else {
    match &claims.aud {
      jwtk::OneOrMany::One(aud) => audiences.contains(aud),
      // if user token has list of aud, validate that at least one of them is inside validation set
      jwtk::OneOrMany::Vec(auds) => auds.iter().any(|aud| audiences.contains(aud)),
    }
  }
}

#[cfg(test)]
mod tests {
  use jwtk::Claims;

  use super::*;

  mod iss {
    use super::*;
    use crate::blueprint::JwtProvider;

    #[test]
    fn validate_iss_not_defined() {
      let options = JwtProvider::test_value();
      let mut claims = Claims::<()>::default();

      assert!(validate_iss(&options, &claims));

      claims.iss = Some("iss".to_owned());

      assert!(validate_iss(&options, &claims));
    }

    #[test]
    fn validate_iss_defined() {
      let options = JwtProvider { issuer: Some("iss".to_owned()), ..JwtProvider::test_value() };
      let mut claims = Claims::<()>::default();

      assert!(!validate_iss(&options, &claims));

      claims.iss = Some("wrong".to_owned());

      assert!(!validate_iss(&options, &claims));

      claims.iss = Some("iss".to_owned());

      assert!(validate_iss(&options, &claims));
    }
  }

  mod aud {
    use std::collections::HashSet;

    use jwtk::OneOrMany;

    use super::*;
    use crate::blueprint::JwtProvider;

    #[test]
    fn validate_aud_not_defined() {
      let options = JwtProvider::test_value();
      let mut claims = Claims::<()>::default();

      assert!(validate_aud(&options, &claims));

      claims.aud = OneOrMany::One("aud".to_owned());

      assert!(validate_aud(&options, &claims));

      claims.aud = OneOrMany::Vec(vec!["aud1".to_owned(), "aud2".to_owned()]);

      assert!(validate_aud(&options, &claims));
    }

    #[test]
    fn validate_aud_defined() {
      let options = JwtProvider {
        audiences: HashSet::from_iter(["aud1".to_owned(), "aud2".to_owned()]),
        ..JwtProvider::test_value()
      };
      let mut claims = Claims::<()>::default();

      assert!(!validate_aud(&options, &claims));

      claims.aud = OneOrMany::One("wrong".to_owned());

      assert!(!validate_aud(&options, &claims));

      claims.aud = OneOrMany::One("aud1".to_owned());

      assert!(validate_aud(&options, &claims));

      claims.aud = OneOrMany::Vec(vec!["aud1".to_owned(), "aud5".to_owned()]);

      assert!(validate_aud(&options, &claims));
    }
  }
}