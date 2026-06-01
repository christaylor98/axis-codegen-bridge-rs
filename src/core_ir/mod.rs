pub mod loader;
mod serialiser;
mod inspect;

pub use loader::{load_core_bundle, load_core_bundle_from_bytes, CoreProgram};
pub use serialiser::{create_core_bundle, write_core_bundle_to_file, create_core_bundle_multi, write_core_bundle_multi_to_file};
pub use inspect::inspect_core_bundle;

use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub enum Provenance {
    Mechanical,
    LlmCandidate,
    BulkCorpus,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EffectClass {
    Pure,
    Reads,
    Writes,
    FullIo,
}

#[derive(Clone, Debug)]
pub struct Span { pub file: String, pub line: usize, pub column: usize }

#[derive(Clone, Debug)]
pub enum CoreTerm {
    IntLit(i64, Option<Span>),
    BoolLit(bool, Option<Span>),
    UnitLit(Option<Span>),
    Var(String, Option<Span>),
    Lam(String, Rc<CoreTerm>, Option<Span>),
    Let(String, Rc<CoreTerm>, Rc<CoreTerm>, Option<Span>),
    If(Rc<CoreTerm>, Rc<CoreTerm>, Rc<CoreTerm>, Option<Span>),
    App(Rc<CoreTerm>, Rc<CoreTerm>, Option<Span>),
    Call(String, Vec<CoreTerm>, Option<Span>),
}
