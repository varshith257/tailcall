use core::future::Future;
use std::fmt::{Debug, Display};
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_graphql::ErrorExtensions;
use async_graphql_value::ConstValue;
use thiserror::Error;

use super::{Concurrent, Eval, EvaluationContext, ResolverContextLike, IO};
use crate::blueprint::DynamicValue;
use crate::json::JsonLike;
use crate::lambda::cache::Cache;
use crate::serde_value_ext::ValueExt;

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

pub trait Execute {
    fn execute<'a, Ctx: ResolverContextLike<'a> + Sync + Send>(
        &'a self,
        ctx: EvaluationContext<'a, Ctx>,
        results: &'a [ConstValue],
    ) -> impl Future<Output = Result<ConstValue>> + 'a + Send;
}

#[derive(Clone, Debug)]
pub enum ExecutableExpression {
    Context(ExecutableContext),
    Dynamic(DynamicValue),
    IO(usize),
    Cache(usize),
    Path(Option<Box<ExecutableExpression>>, Vec<String>),
    Protect(Box<ExecutableExpression>),
}

#[derive(Clone, Debug)]
pub enum ExecutableContext {
    Value,
    Path(Vec<String>),
    PushArgs {
        expr: Option<Box<ExecutableExpression>>,
        and_then: Box<ExecutableExpression>,
    },
    PushValue {
        expr: Option<Box<ExecutableExpression>>,
        and_then: Box<ExecutableExpression>,
    },
}

pub struct Executable {
    expression: ExecutableExpression,
    ios: Vec<IO>,
    caches: Vec<Cache>,
}

impl Expression {
    pub fn and_then(self, next: Self) -> Self {
        Expression::Context(Context::PushArgs { expr: Box::new(self), and_then: Box::new(next) })
    }

    pub fn with_args(self, args: Expression) -> Self {
        Expression::Context(Context::PushArgs { expr: Box::new(args), and_then: Box::new(self) })
    }

    pub fn into_executable(self) -> Executable {
        let ios = vec![];
        let caches = vec![];
        let mut executable = Executable {
            expression: ExecutableExpression::Dynamic(DynamicValue::Value(ConstValue::Null)),
            ios,
            caches,
        };
        let expression = self.into_executable_expression(&mut executable);
        executable.expression = *expression;

        executable
    }

    pub fn into_executable_expression(
        self,
        executable: &mut Executable,
    ) -> Box<ExecutableExpression> {
        Box::new(match self {
            Expression::Context(ctx) => match ctx {
                Context::Value => ExecutableExpression::Context(ExecutableContext::Value),
                Context::Path(path) => ExecutableExpression::Context(ExecutableContext::Path(path)),
                Context::PushArgs { expr, and_then } => {
                    ExecutableExpression::Context(ExecutableContext::PushArgs {
                        expr: Some(expr.as_ref().clone().into_executable_expression(executable)),
                        and_then: and_then
                            .as_ref()
                            .clone()
                            .into_executable_expression(executable),
                    })
                }
                Context::PushValue { expr, and_then } => {
                    ExecutableExpression::Context(ExecutableContext::PushValue {
                        expr: Some(expr.as_ref().clone().into_executable_expression(executable)),
                        and_then: and_then
                            .as_ref()
                            .clone()
                            .into_executable_expression(executable),
                    })
                }
            },
            Expression::Dynamic(value) => ExecutableExpression::Dynamic(value),
            Expression::IO(io) => {
                let id = executable.ios.len();
                executable.ios.push(io);
                ExecutableExpression::IO(id)
            }
            Expression::Cache(cache) => {
                let id = executable.caches.len();
                executable.caches.push(cache);
                ExecutableExpression::Cache(id)
            }
            Expression::Path(expr, path) => ExecutableExpression::Path(
                Some(expr.as_ref().clone().into_executable_expression(executable)),
                path,
            ),
            Expression::Protect(expr) => ExecutableExpression::Protect(
                expr.as_ref().clone().into_executable_expression(executable),
            ),
        })
    }
}

impl Executable {
    pub fn execute<'a, Ctx: ResolverContextLike<'a> + Sync + Send>(
        &'a mut self,
        ctx: EvaluationContext<'a, Ctx>,
    ) -> impl Future<Output = Result<ConstValue>> + 'a + Send {
        let mut execution_stack = vec![(ctx, self.expression.clone())];
        let mut last_result = None;
        async move {
            while let Some((ctx, executable)) = execution_stack.pop() {
                match executable {
                    ExecutableExpression::Context(context) => match context {
                        ExecutableContext::Value => {
                            last_result =
                                Some(ctx.value().cloned().unwrap_or(async_graphql::Value::Null));
                        }
                        ExecutableContext::Path(path) => {
                            last_result = Some(
                                ctx.path_value(&path)
                                    .map(|a| a.into_owned())
                                    .unwrap_or(async_graphql::Value::Null),
                            )
                        }
                        ExecutableContext::PushArgs { expr: Some(expr), and_then } => {
                            let push_args = ExecutableContext::PushArgs { expr: None, and_then };
                            let executable = ExecutableExpression::Context(push_args);
                            execution_stack.push((ctx.clone(), executable));
                            execution_stack.push((ctx.clone(), *expr))
                        }
                        ExecutableContext::PushArgs { expr: None, and_then } => {
                            let ctx = ctx.with_args(last_result.clone().unwrap());
                            execution_stack.push((ctx, *and_then));
                        }
                        ExecutableContext::PushValue { expr: Some(expr), and_then } => {
                            let push_val = ExecutableContext::PushValue { expr: None, and_then };
                            let executable = ExecutableExpression::Context(push_val);
                            execution_stack.push((ctx.clone(), executable));
                            execution_stack.push((ctx.clone(), *expr))
                        }
                        ExecutableContext::PushValue { expr: None, and_then } => {
                            let ctx = ctx.with_value(last_result.clone().unwrap());
                            execution_stack.push((ctx, *and_then));
                        }
                    },
                    ExecutableExpression::Dynamic(value) => {
                        last_result = Some(value.render_value(&ctx)?)
                    }
                    ExecutableExpression::IO(id) => {
                        last_result =
                            Some(self.ios[id].eval_io(ctx, &Concurrent::Sequential).await?)
                    }
                    ExecutableExpression::Cache(id) => {
                        last_result = Some(
                            self.caches[id]
                                .eval_cache(ctx, &Concurrent::Sequential)
                                .await?,
                        )
                    }
                    ExecutableExpression::Path(Some(expr), path) => {
                        execution_stack.push((ctx.clone(), ExecutableExpression::Path(None, path)));
                        execution_stack.push((ctx, *expr))
                    }
                    ExecutableExpression::Path(None, path) => {
                        last_result = Some(
                            last_result
                                .clone()
                                .unwrap()
                                .get_path(&path)
                                .unwrap_or(&async_graphql::Value::Null)
                                .clone(),
                        )
                    }
                    ExecutableExpression::Protect(expr) => {
                        ctx.request_ctx
                            .auth_ctx
                            .validate(ctx.request_ctx)
                            .await
                            .to_result()
                            .map_err(|e| anyhow!("Authentication Failure: {}", e.to_string()))?;
                        execution_stack.push((ctx, *expr))
                    }
                }
            }

            Ok(last_result.unwrap())
        }
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
