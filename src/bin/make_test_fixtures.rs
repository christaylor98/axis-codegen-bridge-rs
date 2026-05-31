/// Generate binary test fixture .coreir files for the link tests.
///
/// Run from the axis-codegen-bridge-rs repo root:
///   cargo run --bin make-test-fixtures
///
/// Produces:
///   tests/lib/double.coreir  — entrypointName="double",  body=lam(x, int_mul(x,2))
///   tests/lib/greet.coreir   — entrypointName="greet",   body=lam(x, io_println(int_to_str(x)))
///   tests/test_link_main.coreir — let d = double(21) in greet(d)

use axis_codegen_bridge::core_ir::{CoreTerm, Provenance, EffectClass, write_core_bundle_to_file};
use std::rc::Rc;

fn main() {
    std::fs::create_dir_all("tests/lib").expect("create tests/lib");

    // double: lam(x, int_mul(x, 2))
    let double_term = CoreTerm::Lam(
        "x".to_string(),
        Rc::new(CoreTerm::Call(
            "int_mul".to_string(),
            vec![
                CoreTerm::Var("x".to_string(), None),
                CoreTerm::IntLit(2, None),
            ],
            None,
        )),
        None,
    );
    write_core_bundle_to_file(
        &double_term,
        "double",
        Provenance::Mechanical,
        EffectClass::Pure,
        true,
        "tests/lib/double.coreir",
    )
    .expect("write double.coreir");
    println!("wrote tests/lib/double.coreir");

    // greet: lam(x, io_println(int_to_str(x)))
    let greet_term = CoreTerm::Lam(
        "x".to_string(),
        Rc::new(CoreTerm::Call(
            "io_println".to_string(),
            vec![CoreTerm::Call(
                "int_to_str".to_string(),
                vec![CoreTerm::Var("x".to_string(), None)],
                None,
            )],
            None,
        )),
        None,
    );
    write_core_bundle_to_file(
        &greet_term,
        "greet",
        Provenance::Mechanical,
        EffectClass::FullIo,
        true,
        "tests/lib/greet.coreir",
    )
    .expect("write greet.coreir");
    println!("wrote tests/lib/greet.coreir");

    // test_link_main: let d = double(21) in greet(d)
    let main_term = CoreTerm::Let(
        "d".to_string(),
        Rc::new(CoreTerm::Call(
            "double".to_string(),
            vec![CoreTerm::IntLit(21, None)],
            None,
        )),
        Rc::new(CoreTerm::Call(
            "greet".to_string(),
            vec![CoreTerm::Var("d".to_string(), None)],
            None,
        )),
        None,
    );
    write_core_bundle_to_file(
        &main_term,
        "test_link_main",
        Provenance::Mechanical,
        EffectClass::FullIo,
        true,
        "tests/test_link_main.coreir",
    )
    .expect("write test_link_main.coreir");
    println!("wrote tests/test_link_main.coreir");
}
