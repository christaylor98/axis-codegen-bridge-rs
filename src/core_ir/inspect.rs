use super::{CoreTerm, loader::load_core_bundle};

pub fn inspect_core_bundle(path: &str) -> Result<String, String> {
    let program = load_core_bundle(path)?;
    Ok(format!(
        "Core bundle: {}\n  Version: 0.4\n  Root: {}",
        path,
        summarise(&program.root_term)
    ))
}

fn summarise(term: &CoreTerm) -> String {
    match term {
        CoreTerm::IntLit(n, _)       => format!("IntLit({})", n),
        CoreTerm::BoolLit(b, _)      => format!("BoolLit({})", b),
        CoreTerm::UnitLit(_)         => "UnitLit".to_string(),
        CoreTerm::Var(n, _)          => format!("Var({})", n),
        CoreTerm::Lam(p, _, _)       => format!("Lam({}, …)", p),
        CoreTerm::App(_, _, _)       => "App(…)".to_string(),
        CoreTerm::Let(n, _, _, _)    => format!("Let({}, …)", n),
        CoreTerm::If(_, _, _, _)     => "If(…)".to_string(),
        CoreTerm::Call(t, args, _)   => format!("Call({}, {} args)", t, args.len()),
    }
}
