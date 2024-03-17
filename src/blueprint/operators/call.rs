use crate::blueprint::*;

use crate::config::Step;
use crate::lambda::Expression;

use crate::valid::{Valid, Validator};

fn compile_call(
    query: &Option<ObjectTypeDefinition>,
    mutation: &Option<ObjectTypeDefinition>,
    steps: &Vec<Step>,
) -> Valid<Expression, String> {
    Valid::from_iter(steps.iter(), |step| match step {
        Step::Operation(name) => {
            let field_def = query
                .as_ref()
                .and_then(|q| q.fields.iter().find(|field| field.name.as_str() == name))
                .or(mutation
                    .as_ref()
                    .and_then(|m| m.fields.iter().find(|field| field.name.as_str() == name)));
            match field_def.and_then(|f| f.resolver.clone()) {
                None => Valid::fail("No operation found".to_string()),
                Some(expr) => Valid::succeed(expr),
            }
        },
        Step::Debug(prefix) => Valid::succeed(Expression::Debug(prefix.clone())),
    })
    .and_then(|exprs| {
        Valid::from_option(
            exprs.into_iter().reduce(|a, b| a.and_then(b)),
            "Operation list is empty".to_string(),
        )
    })
}

pub fn update_call(blueprint: Blueprint) -> Valid<Blueprint, String> {
    let query = blueprint.query_def().cloned();
    let mutation = blueprint.mutation_def().cloned();

    blueprint.modify_def(|object_def| {
        object_def
            .clone()
            .modify_field(|field_def| {
                let field_def = field_def.clone();
                match field_def.source.1.call {
                    None => return Valid::succeed(field_def),
                    Some(ref call) => {
                        let steps = call.clone().steps;
                        let mut field_def = field_def.clone();
                        let name = field_def.name.clone();
                        compile_call(&query, &mutation, &steps)
                            .map(move |expr| {
                                field_def.resolver = Some(expr.clone());
                                field_def
                            })
                            .trace(name.as_str())
                    }
                }
            })
            .trace(object_def.name())
    })
}
