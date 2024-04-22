use core::future::Future;
use std::collections::VecDeque;
use std::fmt::{Debug, Display};
use std::num::NonZeroU64;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_graphql::ErrorExtensions;
use async_graphql_value::ConstValue;
use thiserror::Error;

use crate::blueprint::DynamicValue;
use crate::json::JsonLike;
use crate::lambda::cache::Cache;
use crate::serde_value_ext::ValueExt;

use super::{Concurrent, Eval, EvaluationContext, IO, ResolverContextLike};

#[derive(Clone, Debug)]
pub enum Expression {
    Context(Context),
    Dynamic(DynamicValue),
    IO(IO),
    Cache(Cache),
    Path(Box<Expression>, Vec<String>),
    Protect(Box<Expression>),
}

impl Display for Expression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Expression::Context(_) => write!(f, "Context"),
            Expression::Dynamic(_) => write!(f, "Literal"),
            Expression::IO(io) => write!(f, "{io}"),
            Expression::Cache(_) => write!(f, "Cache"),
            Expression::Path(_, _) => write!(f, "Input"),
            Expression::Protect(expr) => write!(f, "Protected({expr})"),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Context {
    Value,
    Path(Vec<String>),
    PushArgs {
        expr: Box<Expression>,
        and_then: Box<Expression>,
    },
    PushValue {
        expr: Box<Expression>,
        and_then: Box<Expression>,
    },
}

#[derive(Debug, Error, Clone)]
pub enum EvaluationError {
    #[error("IOException: {0}")]
    IOException(String),

    #[error("gRPC Error: status: {grpc_code}, description: `{grpc_description}`, message: `{grpc_status_message}`")]
    GRPCError {
        grpc_code: i32,
        grpc_description: String,
        grpc_status_message: String,
        grpc_status_details: ConstValue,
    },

    #[error("APIValidationError: {0:?}")]
    APIValidationError(Vec<String>),

    #[error("ExprEvalError: {0:?}")]
    ExprEvalError(String),
}

impl From<anyhow::Error> for EvaluationError {
    fn from(value: anyhow::Error) -> Self {
        match value.downcast::<EvaluationError>() {
            Ok(err) => err,
            Err(err) => EvaluationError::IOException(err.to_string()),
        }
    }
}

impl From<Arc<anyhow::Error>> for EvaluationError {
    fn from(error: Arc<anyhow::Error>) -> Self {
        match error.downcast_ref::<EvaluationError>() {
            Some(err) => err.clone(),
            None => EvaluationError::IOException(error.to_string()),
        }
    }
}

impl ErrorExtensions for EvaluationError {
    fn extend(&self) -> async_graphql::Error {
        async_graphql::Error::new(format!("{}", self)).extend_with(|_err, e| {
            if let EvaluationError::GRPCError {
                grpc_code,
                grpc_description,
                grpc_status_message,
                grpc_status_details,
            } = self
            {
                e.set("grpc_code", *grpc_code);
                e.set("grpc_description", grpc_description);
                e.set("grpc_status_message", grpc_status_message);
                e.set("grpc_status_details", grpc_status_details.clone());
            }
        })
    }
}

impl<'a> From<crate::valid::ValidationError<&'a str>> for EvaluationError {
    fn from(_value: crate::valid::ValidationError<&'a str>) -> Self {
        EvaluationError::APIValidationError(
            _value
                .as_vec()
                .iter()
                .map(|e| e.message.to_owned())
                .collect(),
        )
    }
}

pub enum Operation {
    Pending(Executable),
    Output(ConstValue),
}

pub trait Execute {
    async fn execute<'a, Ctx: ResolverContextLike<'a> + Sync + Send>(
        &'a self,
        ctx: EvaluationContext<'a, Ctx>,
        results: &'a Vec<ConstValue>
    ) -> Result<ConstValue>;
}

// impl Execute for Operation {
//     async fn execute<'a, Ctx: ResolverContextLike<'a> + Sync + Send>(&'a self, ctx: EvaluationContext<'a, Ctx>, results: &'a Vec<Operation>) -> Result<ConstValue> {
//         match self {
//             Operation::Pending(executable) => executable.execute(ctx, results).await,
//             Operation::Output(value) => { Ok(value.clone()) }
//         }
//     }
// }

pub enum Executable {
    ExpressionContextValue,
    ExpressionContextPath { path: Vec<String> },
    ExpressionContextPushArgs { expr: usize, and_then: usize },
    ExpressionContextPushValue { expr: usize, and_then: usize },
    ExpressionPath(usize, Vec<String>),
    ExpressionDynamic(DynamicValue),
    ExpressionProtect(usize),
    ExpressionIO(IO),
    ExpressionCache(Cache),
}

impl Execute for Executable {
    async fn execute<'a, Ctx: ResolverContextLike<'a> + Sync + Send>(
        &'a self,
        ctx: EvaluationContext<'a, Ctx>,
        results: &'a Vec<ConstValue>
    ) -> Result<ConstValue> {
        use Executable::*;
        match self {
            ExpressionContextValue => Ok(ctx.value().cloned().unwrap_or(async_graphql::Value::Null)),
            ExpressionContextPath { path } => Ok(ctx
                .path_value(path)
                .map(|a| a.into_owned())
                .unwrap_or(async_graphql::Value::Null)),
            ExpressionContextPushArgs { expr, and_then } => {
                let expr = results[*expr].clone();
                let ctx = ctx.with_args(expr).clone();
                Ok(results[*and_then].clone())
            }
            ExpressionContextPushValue { expr, and_then } => {
                let expr = results[*expr].clone();
                let ctx = ctx.with_value(expr);
                Ok(results[*and_then].clone())
            }
            ExpressionPath(input, path) => {
                let input = results[*input].clone();
                Ok(input
                    .get_path(path)
                    .unwrap_or(&async_graphql::Value::Null)
                    .clone())
            },
            ExpressionDynamic(value) => value.render_value(&ctx),
            ExpressionProtect(expr) => {
                ctx.request_ctx
                    .auth_ctx
                    .validate(ctx.request_ctx)
                    .await
                    .to_result()
                    .map_err(|e| anyhow!("Authentication Failure: {}", e.to_string()))?;
                Ok(results[*expr].clone())
            }
            ExpressionIO(io) => io.eval_inner(ctx, &Concurrent::Sequential).await,
            ExpressionCache(cache) => cache.eval(ctx, &Concurrent::Sequential).await,
        }
    }
}

impl Expression {
    pub fn and_then(self, next: Self) -> Self {
        Expression::Context(Context::PushArgs { expr: Box::new(self), and_then: Box::new(next) })
    }

    pub fn with_args(self, args: Expression) -> Self {
        Expression::Context(Context::PushArgs { expr: Box::new(args), and_then: Box::new(self) })
    }

    pub fn create_operation_list(self) -> Vec<Executable> {
        let mut queue = VecDeque::from([self]);
        let mut index = 0;
        let mut operations = Vec::new();
        while let Some(expr) = queue.pop_front() {
            index += 1;
            let len = index;
            let operation = match expr {
                Expression::Context(op) => match op {
                    Context::Value => Executable::ExpressionContextValue,
                    Context::Path(path) => Executable::ExpressionContextPath { path: path.clone() },
                    Context::PushArgs { expr, and_then } => {
                        queue.push_back(expr.as_ref().clone());
                        queue.push_back(and_then.as_ref().clone());
                        Executable::ExpressionContextPushArgs { expr: len, and_then: len+1 }
                    },
                    Context::PushValue { expr, and_then } => {
                        queue.push_back(expr.as_ref().clone());
                        queue.push_back(and_then.as_ref().clone());
                        Executable::ExpressionContextPushValue { expr: len, and_then: len+1 }
                    },
                },
                Expression::Path(input, path) => {
                    queue.push_back(input.as_ref().clone());
                    Executable::ExpressionPath(len, path.clone())
                },
                Expression::Dynamic(value) => {
                    Executable::ExpressionDynamic(value.clone())
                },
                Expression::Protect(expr) => {
                    queue.push_back(expr.as_ref().clone());
                    Executable::ExpressionProtect(len)
                },
                Expression::IO(io) => Executable::ExpressionIO(io.clone()),
                Expression::Cache(cache) => {
                    Executable::ExpressionCache(cache.clone())
                },
            };
            operations.push(operation);
        }
        operations
    }
}

impl Eval for Expression {
    #[tracing::instrument(skip_all, fields(otel.name = %self), err)]
    fn eval<'a, Ctx: ResolverContextLike<'a> + Sync + Send>(
        &'a self,
        ctx: EvaluationContext<'a, Ctx>,
        conc: &'a Concurrent,
    ) -> Pin<Box<dyn Future<Output = Result<ConstValue>> + 'a + Send>> {
        Box::pin(async move {
            match self {
                Expression::Context(op) => match op {
                    Context::Value => {
                        Ok(ctx.value().cloned().unwrap_or(async_graphql::Value::Null))
                    }
                    Context::Path(path) => Ok(ctx
                        .path_value(path)
                        .map(|a| a.into_owned())
                        .unwrap_or(async_graphql::Value::Null)),
                    Context::PushArgs { expr, and_then } => {
                        let args = expr.eval(ctx.clone(), conc).await?;
                        let ctx = ctx.with_args(args).clone();
                        and_then.eval(ctx, conc).await
                    }
                    Context::PushValue { expr, and_then } => {
                        let value = expr.eval(ctx.clone(), conc).await?;
                        let ctx = ctx.with_value(value);
                        and_then.eval(ctx, conc).await
                    }
                },
                Expression::Path(input, path) => {
                    let inp = &input.eval(ctx, conc).await?;
                    Ok(inp
                        .get_path(path)
                        .unwrap_or(&async_graphql::Value::Null)
                        .clone())
                }
                Expression::Dynamic(value) => value.render_value(&ctx),
                Expression::Protect(expr) => {
                    ctx.request_ctx
                        .auth_ctx
                        .validate(ctx.request_ctx)
                        .await
                        .to_result()
                        .map_err(|e| anyhow!("Authentication Failure: {}", e.to_string()))?;
                    expr.eval(ctx, conc).await
                }
                Expression::IO(operation) => operation.eval(ctx, conc).await,
                Expression::Cache(cached) => cached.eval(ctx, conc).await,
            }
        })
    }
}
