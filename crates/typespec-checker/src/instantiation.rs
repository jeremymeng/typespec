use std::collections::HashMap;

use crate::types::{SymbolId, TypeId};

#[derive(Debug, Default, Clone)]
pub struct InstantiationMap {
    cache: HashMap<(SymbolId, Vec<TypeId>), TypeId>,
}

impl InstantiationMap {
    pub fn get(&self, symbol: SymbolId, args: &[TypeId]) -> Option<TypeId> {
        self.cache.get(&(symbol, args.to_vec())).copied()
    }

    pub fn insert(&mut self, symbol: SymbolId, args: Vec<TypeId>, type_id: TypeId) {
        self.cache.insert((symbol, args), type_id);
    }
}
