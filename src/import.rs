//! Import system for host functions and inter-module calls
//! Provides types and traits for resolving WASM imports

use crate::exec::{Continuation, Fault, Value};
use crate::instance::TableInstance;
use crate::memory::VectorMemory;
use crate::stack::Stack;

/// Host function - a boxed function that takes a stack and returns a continuation
pub type HostFunction = Box<dyn Fn(&mut Stack) -> Result<Continuation, Fault> + Send + Sync>;

/// Resolved import - either a host function or inter-module reference
pub enum ImportedFunction {
    Host(HostFunction),
    Module { module_id: u32, func_idx: u32 },
}

/// Trait for resolving imports when adding modules to ExecutionState
pub trait ImportResolver {
    fn resolve_function(&self, module: &str, field: &str) -> Option<ImportedFunction>;
    fn resolve_global(&self, module: &str, field: &str) -> Option<Value>;
    fn resolve_table(&self, module: &str, field: &str) -> Option<TableInstance>;
    fn resolve_memory(&self, module: &str, field: &str) -> Option<VectorMemory>;
}
