#[cfg(test)]
pub mod tests {
    use std::rc::Rc;
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};
    use tailcall::auth::basic::BasicVerifier;
    use tailcall::auth::error::Error;
    use tailcall::auth::verify::Verify;
    use tailcall::blueprint;
    use tailcall::http::RequestContext;
    use hyper::HeaderMap;
    use tailcall::auth::context::AuthContext;
    use tailcall::blueprint::Server;
    use tailcall::chrono_cache::ChronoCache;
    use tailcall::config::Config;
    use hyper::header::HeaderValue;
    use hyper::http::HeaderName;
    use sha1::Digest;
    use worker::Env;
    use worker::wasm_bindgen::JsValue;
    use cloudflare::{init_env, init_http};

    fn default() -> RequestContext {
            let Config { server, upstream, .. } = Config::default();
            let server = Server::try_from(server);
        let server = server.unwrap();
            let h_client = Arc::new(init_http());
            let h2_client =h_client.clone();
            RequestContext {
                req_headers: HeaderMap::new(),
                allowed_headers: HeaderMap::new(),
                h_client,
                h2_client,
                server,
                upstream,
                http_data_loaders: Arc::new(vec![]),
                gql_data_loaders: Arc::new(vec![]),
                cache: ChronoCache::new(),
                grpc_data_loaders: Arc::new(vec![]),
                min_max_age: Arc::new(Mutex::new(None)),
                cache_public: Arc::new(Mutex::new(None)),
                env_vars: Arc::new(init_env(Rc::new(Env::from(JsValue::null())))),
                auth_ctx: AuthContext::default(),
            }
        }

    // testuser1:password123
    // testuser2:mypassword
    // testuser3:abc123
    pub static HTPASSWD_TEST: &str = "
testuser1:$apr1$e3dp9qh2$fFIfHU9bilvVZBl8TxKzL/
testuser2:$2y$10$wJ/mZDURcAOBIrswCAKFsO0Nk7BpHmWl/XuhF7lNm3gBAFH3ofsuu
testuser3:{SHA}Y2fEjdGT1W6nsLqtJbGUVeUp9e4=
";

    pub fn create_basic_auth_request(username: &str, password: &str) -> RequestContext {
        let mut req_context = default();
        let username = username.to_string();

        req_context
            .req_headers
            .insert(HeaderName::from_str(&username).unwrap(),HeaderValue::from_str(password).unwrap());

        req_context
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn verify_passwords() {
        let provider = BasicVerifier::new(blueprint::BasicProvider { htpasswd: HTPASSWD_TEST.to_owned() });
        wasm_logger::init(wasm_logger::Config::new(log::Level::Debug));
        let rc = default();
        let validation = provider.verify(&rc).await.err();
        assert_eq!(validation, Some(Error::Missing));

/*        let validation = provider
            .verify(&create_basic_auth_request("testuser1", "wrong-password"))
            .await
            .err();
        assert_eq!(validation, Some(Error::Invalid));

        let validation = provider
            .verify(&create_basic_auth_request("testuser1", "password123"))
            .await;
        assert!(validation.is_ok());

        let validation = provider
            .verify(&create_basic_auth_request("testuser2", "mypassword"))
            .await;
        assert!(validation.is_ok());

        let validation = provider.verify(&create_basic_auth_request("testuser3", "abc123")).await;
        assert!(validation.is_ok());*/
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_sha1() {
        let pw = "foo";
        wasm_logger::init(wasm_logger::Config::new(log::Level::Debug));
        assert_eq!([11, 238, 199, 181, 234, 63, 15, 219, 201, 93, 13, 212, 127, 60, 91, 194, 117, 218, 138, 51].to_vec(), sha1::Sha1::digest(pw.as_bytes()).to_vec());
    }
}