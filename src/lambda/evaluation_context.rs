use std::borrow::Cow;
use std::time::Duration;

use async_graphql::{Name, SelectionField, ServerError, Value};
use derive_setters::Setters;
use once_cell::sync::Lazy;
use reqwest::header::HeaderMap;

use super::{EmptyResolverContext, GraphQLOperationContext, ResolverContextLike};
use crate::http::RequestContext;
use crate::lambda::SelectionSetFilterData;

// TODO: rename to ResolverContext
#[derive(Clone, Setters)]
#[setters(strip_option)]
pub struct EvaluationContext<'a, Ctx: ResolverContextLike<'a>> {
  pub req_ctx: &'a RequestContext,
  pub graphql_ctx: &'a Ctx,

  // TODO: JS timeout should be read from server settings
  pub timeout: Duration,
}

static REQUEST_CTX: Lazy<RequestContext> = Lazy::new(RequestContext::default);

impl Default for EvaluationContext<'static, EmptyResolverContext> {
  fn default() -> Self {
    Self::new(&REQUEST_CTX, &EmptyResolverContext)
  }
}

impl<'a, Ctx: ResolverContextLike<'a>> EvaluationContext<'a, Ctx> {
  pub fn new(req_ctx: &'a RequestContext, graphql_ctx: &'a Ctx) -> EvaluationContext<'a, Ctx> {
    Self { timeout: Duration::from_millis(5), req_ctx, graphql_ctx }
  }

  pub fn value(&self) -> Option<&Value> {
    self.graphql_ctx.value()
  }

  pub fn arg<T: AsRef<str>>(&self, path: &[T]) -> Option<&'a Value> {
    let arg = self.graphql_ctx.args()?.get(path[0].as_ref());

    get_path_value(arg?, &path[1..])
  }

  pub fn path_value<T: AsRef<str>>(&self, path: &[T]) -> Option<&'a Value> {
    get_path_value(self.graphql_ctx.value()?, path)
  }

  pub fn headers(&self) -> &HeaderMap {
    &self.req_ctx.req_headers
  }

  pub fn header(&self, key: &str) -> Option<&str> {
    let value = self.headers().get(key)?;

    value.to_str().ok()
  }

  pub fn var(&self, key: &str) -> Option<&str> {
    let vars = &self.req_ctx.server.vars;

    vars.get(key).map(|v| v.as_str())
  }

  pub fn add_error(&self, error: ServerError) {
    self.graphql_ctx.add_error(error)
  }
}

impl<'a, Ctx: ResolverContextLike<'a>> GraphQLOperationContext for EvaluationContext<'a, Ctx> {
  fn selection_set(
    &self,
    selection_set_filter: Option<SelectionSetFilterData>,
    filter_selection_set: bool,
  ) -> Option<String> {
    let selection_set = self.graphql_ctx.field()?.selection_set();

    match (filter_selection_set, selection_set_filter) {
      (true, Some(SelectionSetFilterData { url_obj_fields, field_type, url, url_obj_ids })) => format_selection_set(
        selection_set,
        SelectionSetFilterData { url_obj_fields, field_type, url, url_obj_ids },
      ),
      _ => format_selection_set_unfiltered(selection_set),
    }
  }
}

fn format_selection_set<'a>(
  selection_set: impl Iterator<Item = SelectionField<'a>>,
  selection_set_filter: SelectionSetFilterData,
) -> Option<String> {
  let mut set = selection_set
    .filter_map(|selection_field| {
      selection_set_filter
        .url_obj_fields
        .get(&selection_set_filter.url)
        .and_then(|obj_fields| {
          obj_fields.get(&selection_set_filter.field_type).and_then(|fields| {
            fields
              .iter()
              .find(|(name, _)| name == selection_field.name())
              .map(|(_, child_field_type)| {
                format_selection_field(
                  selection_field,
                  SelectionSetFilterData {
                    url_obj_fields: selection_set_filter.url_obj_fields.clone(),
                    field_type: child_field_type.to_owned(),
                    url: selection_set_filter.url.clone(),
                    url_obj_ids: selection_set_filter.url_obj_ids.clone(),
                  },
                )
              })
          })
        })
    })
    .collect::<Vec<_>>();

  println!("selection set without ids added: {:?}", set);

  if !set.is_empty() {
    for (_, obj_ids) in selection_set_filter.url_obj_ids {
      obj_ids.get(&selection_set_filter.field_type).map(|ids| {
        for id in ids {
          set.extend([id.clone()]);
        }
      });
    }
  }

  if set.is_empty() {
    return None;
  }

  Some(format!("{{ {} }}", set.join(" ")))
}

fn format_selection_set_unfiltered<'a>(selection_set: impl Iterator<Item = SelectionField<'a>>) -> Option<String> {
  let set = selection_set.map(format_selection_field_unfiltered).collect::<Vec<_>>();
  if set.is_empty() {
    return None;
  }
  Some(format!("{{ {} }}", set.join(" ")))
}

fn format_selection_field(field: SelectionField, selection_set_filter: SelectionSetFilterData) -> String {
  let name = field.name();
  let arguments = format_selection_field_arguments(field);
  let selection_set = format_selection_set(field.selection_set(), selection_set_filter);

  if let Some(set) = selection_set {
    format!("{}{} {}", name, arguments, set)
  } else {
    format!("{}{}", name, arguments)
  }
}

fn format_selection_field_unfiltered(field: SelectionField) -> String {
  let name = field.name();
  let arguments = format_selection_field_arguments(field);
  let selection_set = format_selection_set_unfiltered(field.selection_set());

  if let Some(set) = selection_set {
    format!("{}{} {}", name, arguments, set)
  } else {
    format!("{}{}", name, arguments)
  }
}

fn format_selection_field_arguments(field: SelectionField) -> Cow<'static, str> {
  let name = field.name();
  let arguments = field
    .arguments()
    .map_err(|error| {
      log::warn!("Failed to resolve arguments for field {name}, due to error: {error}");
      error
    })
    .unwrap_or_default();

  if arguments.is_empty() {
    return Cow::Borrowed("");
  }

  let args = arguments
    .iter()
    .map(|(name, value)| format!("{}: {}", name, value))
    .collect::<Vec<_>>()
    .join(",");

  Cow::Owned(format!("({})", args))
}

pub fn get_path_value<'a, T: AsRef<str>>(input: &'a Value, path: &[T]) -> Option<&'a Value> {
  let mut value = Some(input);
  for name in path {
    match value {
      Some(Value::Object(map)) => {
        value = map.get(&Name::new(name));
      }

      Some(Value::List(list)) => {
        value = list.get(name.as_ref().parse::<usize>().ok()?);
      }
      _ => return None,
    }
  }

  value
}

#[cfg(test)]
mod tests {
  use async_graphql::Value;
  use serde_json::json;

  use crate::lambda::evaluation_context::get_path_value;

  #[test]
  fn test_path_value() {
    let json = json!(
    {
        "a": {
            "b": {
                "c": "d"
            }
        }
    });

    let async_value = Value::from_json(json).unwrap();

    let path = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let result = get_path_value(&async_value, &path);
    assert!(result.is_some());
    assert_eq!(result.unwrap(), &Value::String("d".to_string()));
  }

  #[test]
  fn test_path_not_found() {
    let json = json!(
    {
        "a": {
            "b": "c"
        }
    });

    let async_value = Value::from_json(json).unwrap();

    let path = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let result = get_path_value(&async_value, &path);
    assert!(result.is_none());
  }

  #[test]
  fn test_numeric_path() {
    let json = json!(
    {
        "a": [{
            "b": "c"
        }]
    });

    let async_value = Value::from_json(json).unwrap();

    let path = vec!["a".to_string(), "0".to_string(), "b".to_string()];
    let result = get_path_value(&async_value, &path);
    assert!(result.is_some());
    assert_eq!(result.unwrap(), &Value::String("c".to_string()));
  }
}
