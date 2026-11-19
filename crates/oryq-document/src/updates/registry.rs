//! Owner-local update operator trait and registry (`docs/modules/document.md` §4).

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;

use oryq_types::Result;

use crate::types::Value;
use crate::updates::inc::IncOperator;
use crate::updates::pull::PullOperator;
use crate::updates::push::PushOperator;
use crate::updates::set::SetOperator;
use crate::updates::unset::UnsetOperator;

/// Pure update operator evaluated against a mutable top-level field vector.
pub trait UpdateOperator: Debug + Send + Sync {
    /// Operator keyword including leading `$` (e.g. `"$set"`, `"$inc"`).
    fn name(&self) -> &'static str;

    /// Applies this operator at validated `segments` with `operand`, returning `true` if modified.
    fn apply_field(
        &self,
        doc: &mut Vec<(String, Value)>,
        segments: &[&str],
        operand: &Value,
    ) -> Result<bool>;
}

/// Owner-local registry of supported update operators.
#[derive(Debug, Default)]
pub struct UpdateRegistry {
    operators: HashMap<&'static str, Arc<dyn UpdateOperator>>,
}

impl UpdateRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers all baseline v1 update operators (`$set`, `$unset`, `$inc`, `$push`, `$pull`).
    pub fn default_v1() -> Self {
        let mut reg = Self::new();
        reg.register(Arc::new(SetOperator));
        reg.register(Arc::new(UnsetOperator));
        reg.register(Arc::new(IncOperator));
        reg.register(Arc::new(PushOperator));
        reg.register(Arc::new(PullOperator));
        reg
    }

    pub fn register(&mut self, op: Arc<dyn UpdateOperator>) {
        self.operators.insert(op.name(), op);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn UpdateOperator>> {
        self.operators.get(name)
    }
}
