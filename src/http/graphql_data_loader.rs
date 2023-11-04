use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_graphql::async_trait;
use async_graphql::dataloader::{DataLoader, Loader, NoCache};
use async_graphql::futures_util::future::join_all;
use reqwest::Url;

use crate::config::Batch;
use crate::http::{DataLoaderRequest, HttpClient, Response};

pub struct GraphqlDataLoader<C>
where
  C: HttpClient + Send + Sync + 'static + Clone,
{
  pub client: C,
}
impl<C: HttpClient + Send + Sync + 'static + Clone> GraphqlDataLoader<C> {
  pub fn new(client: C) -> Self {
    GraphqlDataLoader { client }
  }

  pub fn to_data_loader(self, batch: Batch) -> DataLoader<GraphqlDataLoader<C>, NoCache> {
    DataLoader::new(self, tokio::spawn)
      .delay(Duration::from_millis(batch.delay as u64))
      .max_batch_size(batch.max_size)
  }
}

#[async_trait::async_trait]
impl<C: HttpClient + Send + Sync + 'static + Clone> Loader<DataLoaderRequest> for GraphqlDataLoader<C> {
  type Value = Response;
  type Error = Arc<anyhow::Error>;

  #[allow(clippy::mutable_key_type)]
  async fn load(
    &self,
    keys: &[DataLoaderRequest],
  ) -> async_graphql::Result<HashMap<DataLoaderRequest, Self::Value>, Self::Error> {
    let request_groups = group_requests_by_url(keys);

    let results = request_groups.iter().map(|(group_key, requests_in_group)| async move {
      let batched_req = create_batched_request(requests_in_group);
      let result = self.client.execute(batched_req).await;
      (group_key.clone(), result)
    });
    let results = join_all(results).await;

    #[allow(clippy::mutable_key_type)]
    let hashmap = extract_individual_responses(results, request_groups);
    Ok(hashmap)
  }
}

fn group_requests_by_url(dataloader_requests: &[DataLoaderRequest]) -> HashMap<Url, Vec<DataLoaderRequest>> {
  let mut batch_key_groups: HashMap<Url, Vec<DataLoaderRequest>> = HashMap::new();
  for dataloader_req in dataloader_requests.iter() {
    if let Some(mut group) = batch_key_groups.get(dataloader_req.to_request().url()).cloned() {
      group.push(dataloader_req.clone());
      batch_key_groups.insert(dataloader_req.to_request().url().clone(), group);
    } else {
      batch_key_groups.insert(dataloader_req.to_request().url().clone(), vec![dataloader_req.clone()]);
    }
  }
  batch_key_groups
}

fn collect_request_bodies(dataloader_requests: &[DataLoaderRequest]) -> String {
  let batched_query = dataloader_requests
    .iter()
    .map(|dataloader_req| {
      if let Some(body) = dataloader_req.to_request().body() {
        if let Some(bytes) = body.as_bytes() {
          if let Ok(body_str) = std::str::from_utf8(bytes) {
            body_str.to_string().clone()
          } else {
            "".to_string()
          }
        } else {
          "".to_string()
        }
      } else {
        "".to_string()
      }
    })
    .collect::<Vec<_>>()
    .join(",");
  format!("[{}]", batched_query)
}

fn create_batched_request(dataloader_requests: &[DataLoaderRequest]) -> reqwest::Request {
  let batched_query = collect_request_bodies(dataloader_requests);

  let first_req = dataloader_requests.get(0).unwrap(); // TODO fix!
  let mut batched_req = first_req.to_request();
  batched_req.body_mut().replace(reqwest::Body::from(batched_query));
  batched_req
}

#[allow(clippy::mutable_key_type)]
fn extract_individual_responses(
  results: Vec<(Url, Result<Response, anyhow::Error>)>,
  request_groups: HashMap<Url, Vec<DataLoaderRequest>>,
) -> HashMap<DataLoaderRequest, Response> {
  #[allow(clippy::mutable_key_type)]
  let mut hashmap = HashMap::new();
  for (key, result) in results {
    let group = request_groups.get(&key);
    if let Ok(res) = result {
      if let async_graphql_value::ConstValue::List(values) = res.body {
        for (i, request) in group.unwrap_or(&Vec::new()).iter().enumerate() {
          let value = values.get(i).unwrap_or(&async_graphql_value::ConstValue::Null);
          hashmap.insert(
            request.clone(),
            Response { status: res.status, headers: res.headers.clone(), body: value.clone() },
          );
        }
      }
    }
  }
  hashmap
}

#[cfg(test)]
mod tests {
  use std::collections::BTreeSet;
  use std::sync::atomic::{AtomicUsize, Ordering};
  use std::sync::{Arc, Mutex};

  use super::*;
  use crate::http::DataLoaderRequest;

  #[derive(Clone)]
  struct MockHttpClient {
    // To keep track of number of times execute is called
    request_count: Arc<AtomicUsize>,
    request_bodies: Arc<Mutex<Vec<String>>>,
  }

  #[async_trait::async_trait]
  impl HttpClient for MockHttpClient {
    async fn execute(&self, req: reqwest::Request) -> anyhow::Result<Response> {
      self.request_count.fetch_add(1, Ordering::SeqCst);
      let body_str = std::str::from_utf8(req.body().unwrap().as_bytes().unwrap())
        .unwrap()
        .to_string();
      self.request_bodies.lock().unwrap().push(body_str);
      // You can mock the actual response as per your need
      Ok(Response::default())
    }
  }

  #[test]
  fn test_group_requests_by_url() {
    let url1 = Url::parse("http://example1.com").unwrap();
    let url2 = Url::parse("http://example2.com").unwrap();

    let mut request1 = reqwest::Request::new(reqwest::Method::GET, url1.clone());
    request1.body_mut().replace(reqwest::Body::from("a".to_string()));
    let mut request2 = reqwest::Request::new(reqwest::Method::GET, url1.clone());
    request2.body_mut().replace(reqwest::Body::from("b".to_string()));
    let mut request3 = reqwest::Request::new(reqwest::Method::GET, url2.clone());
    request3.body_mut().replace(reqwest::Body::from("c".to_string()));

    let dl_req1 = DataLoaderRequest::new(request1, BTreeSet::new());
    let dl_req2 = DataLoaderRequest::new(request2, BTreeSet::new());
    let dl_req3 = DataLoaderRequest::new(request3, BTreeSet::new());

    let grouped = group_requests_by_url(&[dl_req1, dl_req2, dl_req3]);

    assert_eq!(grouped.keys().len(), 2);
    assert_eq!(grouped.get(&url1.clone()).unwrap().len(), 2);
    assert_eq!(grouped.get(&url2.clone()).unwrap().len(), 1);
  }

  #[test]
  fn test_collect_request_bodies() {
    let url = Url::parse("http://example.com").unwrap();
    let mut request1 = reqwest::Request::new(reqwest::Method::GET, url.clone());
    request1.body_mut().replace(reqwest::Body::from("a".to_string()));
    let mut request2 = reqwest::Request::new(reqwest::Method::GET, url.clone());
    request2.body_mut().replace(reqwest::Body::from("b".to_string()));
    let mut request3 = reqwest::Request::new(reqwest::Method::GET, url.clone());
    request3.body_mut().replace(reqwest::Body::from("c".to_string()));

    let dl_req1 = DataLoaderRequest::new(request1, BTreeSet::new());
    let dl_req2 = DataLoaderRequest::new(request2, BTreeSet::new());
    let dl_req3 = DataLoaderRequest::new(request3, BTreeSet::new());

    let body = collect_request_bodies(&[dl_req1, dl_req2, dl_req3]);
    assert_eq!(body, "[a,b,c]");
  }

  #[tokio::test]
  async fn test_load_function() {
    let client =
      MockHttpClient { request_count: Arc::new(AtomicUsize::new(0)), request_bodies: Arc::new(Mutex::new(Vec::new())) };
    let loader = GraphqlDataLoader { client: client.clone() };
    let loader = loader.to_data_loader(Batch::default().delay(1));

    let url = Url::parse("http://example.com").unwrap();
    let mut request = reqwest::Request::new(reqwest::Method::GET, url.clone());
    request.body_mut().replace(reqwest::Body::from("a".to_string()));
    let key = DataLoaderRequest::new(request, BTreeSet::new());
    let futures: Vec<_> = (0..100).map(|_| loader.load_one(key.clone())).collect();
    let _ = join_all(futures).await;
    assert_eq!(
      client.request_count.load(Ordering::SeqCst),
      1,
      "Only one request should be made for the same key"
    );
  }

  #[tokio::test]
  async fn test_load_many_function() {
    let client =
      MockHttpClient { request_count: Arc::new(AtomicUsize::new(0)), request_bodies: Arc::new(Mutex::new(Vec::new())) };
    let loader = GraphqlDataLoader { client: client.clone() };
    let loader = loader.to_data_loader(Batch::default().delay(1));

    let url = Url::parse("http://example.com").unwrap();
    let futures: Vec<_> = (1..10)
      .map(|i| {
        let mut request = reqwest::Request::new(reqwest::Method::GET, url.clone());
        request.body_mut().replace(reqwest::Body::from(format!("a{}", i)));
        let key = DataLoaderRequest::new(request, BTreeSet::new());
        loader.load_one(key.clone())
      })
      .collect();
    let _ = join_all(futures).await;
    let mut request_body = client.request_bodies.lock().unwrap()[0].clone();
    request_body.pop();
    if !request_body.is_empty() {
      request_body.remove(0);
    }
    let mut req_bodies = request_body.split(',').collect::<Vec<_>>();
    req_bodies.sort();

    assert_eq!(
      client.request_count.load(Ordering::SeqCst),
      1,
      "Only one request should be made for the same url"
    );
    assert_eq!(
      client.request_bodies.lock().unwrap().len(),
      1,
      "Only one request body should be present"
    );
    assert_eq!(
      req_bodies,
      ["a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8", "a9"],
      "Individual request bodies should be joined"
    );
  }
}