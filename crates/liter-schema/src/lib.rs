//! Schema management and catalog for Liter-rs.
//!
//! Mirrors the schema tables in `sqlite_schema` / `sqlite_master`. Stores
//! parsed table, index, view, and trigger definitions and supports fast
//! lookup by name.
//!
//! ## Status
//! Phase 3 — stub skeleton.

use parking_lot::RwLock;
use std::collections::HashMap;

/// The type of a schema object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Table,
    Index,
    View,
    Trigger,
}

/// A single entry in the schema catalog.
#[derive(Debug, Clone)]
pub struct SchemaObject {
    pub kind: ObjectKind,
    pub name: String,
    pub tbl_name: String,
    pub root_page: u32,
    pub sql: Option<String>,
    pub columns: Vec<liter_ast::ColumnDef>,
}

/// The schema catalog for a single database (main, temp, or attached).
#[derive(Debug, Default)]
pub struct Schema {
    objects: RwLock<HashMap<String, SchemaObject>>,
}

impl Schema {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, obj: SchemaObject) {
        self.objects.write().insert(obj.name.clone(), obj);
    }

    pub fn get(&self, name: &str) -> Option<SchemaObject> {
        self.objects.read().get(name).cloned()
    }

    pub fn remove(&self, name: &str) -> Option<SchemaObject> {
        self.objects.write().remove(name)
    }

    pub fn all(&self) -> Vec<SchemaObject> {
        self.objects.read().values().cloned().collect()
    }

    pub fn tables(&self) -> Vec<SchemaObject> {
        self.all()
            .into_iter()
            .filter(|o| o.kind == ObjectKind::Table)
            .collect()
    }

    pub fn indexes_for(&self, table: &str) -> Vec<SchemaObject> {
        self.all()
            .into_iter()
            .filter(|o| o.kind == ObjectKind::Index && o.tbl_name == table)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_table(name: &str) -> SchemaObject {
        SchemaObject {
            kind: ObjectKind::Table,
            name: name.to_owned(),
            tbl_name: name.to_owned(),
            root_page: 2,
            sql: Some(format!("CREATE TABLE {name} (id INTEGER PRIMARY KEY)")),
            columns: vec![liter_ast::ColumnDef {
                name: "id".to_string(),
                type_name: None,
                constraints: vec![],
            }],
        }
    }

    #[test]
    fn insert_and_get() {
        let s = Schema::new();
        s.insert(make_table("users"));
        let obj = s.get("users").unwrap();
        assert_eq!(obj.kind, ObjectKind::Table);
    }

    #[test]
    fn remove() {
        let s = Schema::new();
        s.insert(make_table("tmp"));
        assert!(s.remove("tmp").is_some());
        assert!(s.get("tmp").is_none());
    }

    #[test]
    fn tables_filter() {
        let s = Schema::new();
        s.insert(make_table("a"));
        s.insert(SchemaObject {
            kind: ObjectKind::Index,
            name: "idx_a".to_owned(),
            tbl_name: "a".to_owned(),
            root_page: 3,
            sql: None,
            columns: vec![],
        });
        assert_eq!(s.tables().len(), 1);
    }
}
