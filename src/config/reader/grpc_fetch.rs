use anyhow::{Context, Result};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use hyper::header::HeaderName;
use nom::AsBytes;
use prost::Message;
use prost_reflect::prost_types::{FileDescriptorProto, FileDescriptorSet};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::ConfigReaderContext;
use crate::grpc::protobuf::{ProtobufOperation, ProtobufSet};
use crate::grpc::RequestTemplate;
use crate::mustache::Mustache;
use crate::runtime::TargetRuntime;

const REFLECTION_PROTO: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/proto/reflection.proto"
));

/// This function is just used for better exception handling
fn get_protobuf_set() -> Result<ProtobufSet> {
    let descriptor = protox_parse::parse("reflection", REFLECTION_PROTO)?;
    let mut descriptor_set = FileDescriptorSet::default();
    descriptor_set.file.push(descriptor);
    ProtobufSet::from_proto_file(&descriptor_set)
}

#[derive(Debug, Serialize, Deserialize)]
struct Service {
    name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ListServicesResponse {
    service: Vec<Service>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileDescriptorProtoResponse {
    file_descriptor_proto: Vec<String>,
}

impl FileDescriptorProtoResponse {
    fn get(self) -> Result<Vec<u8>> {
        let file_descriptor_proto = self
            .file_descriptor_proto
            .first()
            .context("Received empty fileDescriptorProto")?;

        Ok(BASE64_STANDARD.decode(file_descriptor_proto)?)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CustomResponse {
    list_services_response: Option<ListServicesResponse>,
    file_descriptor_response: Option<FileDescriptorProtoResponse>,
}

/// Makes `ListService` request to the grpc reflection server
pub async fn list_all_files(url: &str, target_runtime: &TargetRuntime) -> Result<Vec<String>> {
    let protobuf_set = get_protobuf_set()?;

    let grpc_method = "grpc.reflection.v1alpha.ServerReflection.ServerReflectionInfo".try_into()?;

    let reflection_service = protobuf_set.find_service(&grpc_method)?;
    let operation = reflection_service.find_operation(&grpc_method)?;

    let mut url: url::Url = url.parse()?;
    url.set_path("grpc.reflection.v1alpha.ServerReflection/ServerReflectionInfo");

    let req_template = RequestTemplate {
        url: Mustache::parse(url.as_str())?,
        headers: vec![(
            HeaderName::from_static("content-type"),
            Mustache::parse("application/grpc+proto")?,
        )],
        body: Mustache::parse(json!({"list_services": ""}).to_string().as_str()).ok(),
        operation: operation.clone(),
        operation_type: Default::default(),
    };

    let ctx = ConfigReaderContext {
        runtime: target_runtime,
        vars: &Default::default(),
        headers: Default::default(),
    };

    let req = req_template.render(&ctx)?.to_request()?;

    let resp = target_runtime.http.execute(req).await?;
    let body = resp.body.as_bytes();

    let response: CustomResponse = operation.convert_output(body)?;

    // Extracting names from services
    let methods: Vec<String> = response
        .list_services_response
        .context("Expected listServicesResponse but found none")?
        .service
        .iter()
        .map(|s| s.name.clone())
        .collect();

    Ok(methods)
}

/// Makes `Get Service` request to the grpc reflection server
pub async fn get_by_service(
    url: &str,
    target_runtime: &TargetRuntime,
    service: &str,
) -> Result<FileDescriptorProto> {
    let protobuf_set = get_protobuf_set()?;

    let grpc_method = "grpc.reflection.v1alpha.ServerReflection.ServerReflectionInfo".try_into()?;

    let reflection_service = protobuf_set.find_service(&grpc_method)?;
    let operation = reflection_service.find_operation(&grpc_method)?;

    let mut url: url::Url = url.parse()?;
    url.set_path("grpc.reflection.v1alpha.ServerReflection/ServerReflectionInfo");

    let req_template = RequestTemplate {
        url: Mustache::parse(url.as_str())?,
        headers: vec![(
            HeaderName::from_static("content-type"),
            Mustache::parse("application/grpc+proto")?,
        )],
        body: Mustache::parse(
            json!({"file_containing_symbol": service})
                .to_string()
                .as_str(),
        )
        .ok(),
        operation: operation.clone(),
        operation_type: Default::default(),
    };

    let ctx = ConfigReaderContext {
        runtime: target_runtime,
        vars: &Default::default(),
        headers: Default::default(),
    };

    let req = req_template.render(&ctx)?.to_request()?;

    request_proto(req, target_runtime, operation).await
}

/// Makes `Get Proto/Symbol Name` request to the grpc reflection server
pub async fn get_by_proto_name(
    url: &str,
    target_runtime: &TargetRuntime,
    proto_name: &str,
) -> Result<FileDescriptorProto> {
    let protobuf_set = get_protobuf_set()?;

    let grpc_method = "grpc.reflection.v1alpha.ServerReflection.ServerReflectionInfo".try_into()?;

    let reflection_service = protobuf_set.find_service(&grpc_method)?;
    let operation = reflection_service.find_operation(&grpc_method)?;

    let mut url: url::Url = url.parse()?;
    url.set_path("grpc.reflection.v1alpha.ServerReflection/ServerReflectionInfo");

    let req_template = RequestTemplate {
        url: Mustache::parse(url.as_str())?,
        headers: vec![(
            HeaderName::from_static("content-type"),
            Mustache::parse("application/grpc+proto")?,
        )],
        body: Mustache::parse(json!({"file_by_filename": proto_name}).to_string().as_str()).ok(),
        operation: operation.clone(),
        operation_type: Default::default(),
    };

    let ctx = ConfigReaderContext {
        runtime: target_runtime,
        vars: &Default::default(),
        headers: Default::default(),
    };

    let req = req_template.render(&ctx)?.to_request()?;

    request_proto(req, target_runtime, operation).await
}

async fn request_proto(
    req: reqwest::Request,
    target_runtime: &TargetRuntime,
    operation: ProtobufOperation,
) -> Result<FileDescriptorProto> {
    let resp = target_runtime.http.execute(req).await?;
    let body = resp.body.as_bytes();

    let response: CustomResponse = operation.convert_output(body)?;

    let file_descriptor_resp = response
        .file_descriptor_response
        .context("Expected fileDescriptorResponse but found none")?;
    let file_descriptor_proto =
        FileDescriptorProto::decode(file_descriptor_resp.get()?.as_bytes())?;

    Ok(file_descriptor_proto)
}

#[cfg(test)]
mod grpc_fetch {
    use std::path::PathBuf;

    use anyhow::Result;

    use crate::config::reader::grpc_fetch::{get_by_proto_name, get_by_service, list_all_files};

    const NEWS_PROTO: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fake_descriptor.bin"
    ));

    const REFLECTION_LIST_ALL: &[u8] = &[
        0, 0, 0, 0, 70, 18, 2, 58, 0, 50, 64, 10, 18, 10, 16, 110, 101, 119, 115, 46, 78, 101, 119,
        115, 83, 101, 114, 118, 105, 99, 101, 10, 42, 10, 40, 103, 114, 112, 99, 46, 114, 101, 102,
        108, 101, 99, 116, 105, 111, 110, 46, 118, 49, 97, 108, 112, 104, 97, 46, 83, 101, 114,
        118, 101, 114, 82, 101, 102, 108, 101, 99, 116, 105, 111, 110,
    ];

    fn start_mock_server() -> httpmock::MockServer {
        httpmock::MockServer::start()
    }
    #[tokio::test]
    async fn test_resp_file_name() -> Result<()> {
        let server = start_mock_server();

        let http_reflection_file_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/grpc.reflection.v1alpha.ServerReflection/ServerReflectionInfo")
                .body("\0\0\0\0\x0c\x1a\nnews.proto");
            then.status(200).body(NEWS_PROTO);
        });

        let runtime = crate::runtime::test::init(None);
        let resp = get_by_proto_name(
            &format!("http://localhost:{}", server.port()),
            &runtime,
            "news.proto",
        )
        .await?;
        let mut news_proto = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        news_proto.push("src");
        news_proto.push("grpc");
        news_proto.push("tests");
        news_proto.push("proto");
        news_proto.push("news.proto");

        let content = runtime.file.read(news_proto.to_str().unwrap()).await?;
        let expected = protox_parse::parse("news.proto", &content)?;

        assert_eq!(expected.name(), resp.name());

        http_reflection_file_mock.assert();
        Ok(())
    }

    #[tokio::test]
    async fn test_resp_service() -> Result<()> {
        let server = start_mock_server();

        let http_reflection_file_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/grpc.reflection.v1alpha.ServerReflection/ServerReflectionInfo")
                .body("\0\0\0\0\x12\"\x10news.NewsService");
            then.status(200).body(NEWS_PROTO);
        });

        let runtime = crate::runtime::test::init(None);
        let resp = get_by_service(
            &format!("http://localhost:{}", server.port()),
            &runtime,
            "news.NewsService",
        )
        .await?;
        let mut news_proto = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        news_proto.push("src");
        news_proto.push("grpc");
        news_proto.push("tests");
        news_proto.push("proto");
        news_proto.push("news.proto");

        let content = runtime.file.read(news_proto.to_str().unwrap()).await?;
        let expected = protox_parse::parse("news.proto", &content)?;

        assert_eq!(expected.name(), resp.name());

        http_reflection_file_mock.assert();
        Ok(())
    }

    #[tokio::test]
    async fn test_resp_list_all() -> Result<()> {
        let server = start_mock_server();

        let http_reflection_list_all = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/grpc.reflection.v1alpha.ServerReflection/ServerReflectionInfo")
                .body("\0\0\0\0\x02:\0");
            then.status(200).body(REFLECTION_LIST_ALL);
        });

        let runtime = crate::runtime::test::init(None);
        let resp = list_all_files(&format!("http://localhost:{}", server.port()), &runtime).await?;

        assert_eq!(
            [
                "news.NewsService".to_string(),
                "grpc.reflection.v1alpha.ServerReflection".to_string()
            ]
            .to_vec(),
            resp
        );

        http_reflection_list_all.assert();

        Ok(())
    }
    #[tokio::test]
    async fn test_list_all_files_empty_response() -> Result<()> {
        let server = start_mock_server();

        let http_reflection_list_all_empty = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/grpc.reflection.v1alpha.ServerReflection/ServerReflectionInfo")
                .body("\0\0\0\0\x02:\0");
            then.status(200).body("\0\0\0\0\x02:\0"); // Mock an empty response
        });

        let runtime = crate::runtime::test::init(None);
        let resp = list_all_files(&format!("http://localhost:{}", server.port()), &runtime).await;

        assert_eq!(
            "Expected listServicesResponse but found none",
            resp.err().unwrap().to_string()
        );

        http_reflection_list_all_empty.assert();

        Ok(())
    }

    #[tokio::test]
    async fn test_get_by_service_not_found() -> Result<()> {
        let server = start_mock_server();

        let http_reflection_service_not_found = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/grpc.reflection.v1alpha.ServerReflection/ServerReflectionInfo");
            then.status(404); // Mock a 404 not found response
        });

        let runtime = crate::runtime::test::init(None);
        let result = get_by_service(
            &format!("http://localhost:{}", server.port()),
            &runtime,
            "nonexistent.Service",
        )
        .await;

        assert!(result.is_err());

        http_reflection_service_not_found.assert();

        Ok(())
    }

    #[tokio::test]
    async fn test_get_by_proto_name_not_found() -> Result<()> {
        let server = start_mock_server();

        let http_reflection_proto_not_found = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/grpc.reflection.v1alpha.ServerReflection/ServerReflectionInfo");
            then.status(404); // Mock a 404 not found response
        });

        let runtime = crate::runtime::test::init(None);
        let result = get_by_proto_name(
            &format!("http://localhost:{}", server.port()),
            &runtime,
            "nonexistent.proto",
        )
        .await;

        assert!(result.is_err());

        http_reflection_proto_not_found.assert();

        Ok(())
    }
}