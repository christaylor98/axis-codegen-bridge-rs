use std::collections::HashMap;
use crate::core_ir::{CoreProgram, CoreTerm};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value { Unit, Int(i64), Bool(bool) }

#[derive(Debug)]
pub enum RuntimeError {
    MissingFunctionBinding { target_name: String },
    ArityMismatch { target_name: String, expected: usize, got: usize },
    FunctionCallFailed { target_name: String, message: Box<str> },
    MalformedCoreIr { message: Box<str> },
}

pub type FunctionImpl = fn(&[Value]) -> Result<Value, RuntimeError>;

struct FunctionEntry { arity: usize, func: FunctionImpl }

pub struct FunctionProvider { functions: HashMap<String, FunctionEntry> }

impl FunctionProvider {
    pub fn new(entries: Vec<(&str, usize, FunctionImpl)>) -> Self {
        let mut functions = HashMap::new();
        for (name, arity, func) in entries {
            functions.insert(name.to_string(), FunctionEntry { arity, func });
        }
        FunctionProvider { functions }
    }

    pub fn call(&self, target: &str, args: &[Value]) -> Result<Value, RuntimeError> {
        match self.functions.get(target) {
            None => Err(RuntimeError::MissingFunctionBinding { target_name: target.to_string() }),
            Some(e) => {
                if e.arity != args.len() {
                    return Err(RuntimeError::ArityMismatch { target_name: target.to_string(), expected: e.arity, got: args.len() });
                }
                (e.func)(args)
            }
        }
    }
}

pub fn execute_core_program(program: &CoreProgram, registry: &FunctionProvider) -> Result<Value, RuntimeError> {
    eval(&program.root_term, registry, &mut Vec::new())
}

fn eval(term: &CoreTerm, reg: &FunctionProvider, env: &mut Vec<HashMap<String, Value>>) -> Result<Value, RuntimeError> {
    match term {
        CoreTerm::IntLit(n, _)  => Ok(Value::Int(*n)),
        CoreTerm::BoolLit(b, _) => Ok(Value::Bool(*b)),
        CoreTerm::UnitLit(_)    => Ok(Value::Unit),
        CoreTerm::Var(name, _) => {
            for frame in env.iter().rev() {
                if let Some(v) = frame.get(name) { return Ok(v.clone()); }
            }
            Err(RuntimeError::MalformedCoreIr { message: format!("unbound: {}", name).into() })
        }
        CoreTerm::Let(name, val, body, _) => {
            let v = eval(val, reg, env)?;
            let mut frame = HashMap::new();
            frame.insert(name.clone(), v);
            env.push(frame);
            let r = eval(body, reg, env);
            env.pop();
            r
        }
        CoreTerm::If(cond, then, els, _) => match eval(cond, reg, env)? {
            Value::Bool(true)  => eval(then, reg, env),
            Value::Bool(false) => eval(els,  reg, env),
            _ => Err(RuntimeError::MalformedCoreIr { message: "if cond not Bool".into() }),
        },
        CoreTerm::Call(target, args, _) => {
            let mut evaled = Vec::with_capacity(args.len());
            for a in args { evaled.push(eval(a, reg, env)?); }
            reg.call(target, &evaled)
        }
        CoreTerm::Lam(_, _, _) | CoreTerm::App(_, _, _) =>
            Err(RuntimeError::MalformedCoreIr { message: "Lam/App not supported in test executor".into() }),
    }
}
