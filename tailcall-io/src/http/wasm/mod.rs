use std::sync::Arc;

use tailcall::HttpIO;

mod http_io;

pub fn init_http() -> Arc<dyn HttpIO> {
    Arc::new(http_io::CloudflareHttp::init())
}
