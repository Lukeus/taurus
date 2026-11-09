//! Against the servers actually installed, when they are. Ignored by default:
//! CI machines don't have them, and a server's wording isn't ours to pin.
//! Run with `cargo test -p taurus-lsp --test real_servers -- --ignored`.

use std::time::Duration;

use taurus_lsp::{builtin_specs, Lsp, Outcome, Severity};

async fn check(file: &str, text: &str) -> Option<Outcome> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(file);
    std::fs::write(&path, text).unwrap();
    let lsp = Lsp::new(dir.path(), builtin_specs(), Duration::from_secs(20));
    let outcome = lsp.check(&path, text).await;
    lsp.shutdown().await;
    (outcome != Outcome::NotCovered).then_some(outcome)
}

fn has_error(outcome: &Outcome) -> bool {
    matches!(outcome, Outcome::Checked { diagnostics, .. }
        if diagnostics.iter().any(|d| d.severity == Severity::Error))
}

#[tokio::test]
#[ignore = "needs clangd"]
async fn clangd_finds_an_undeclared_name() {
    let Some(outcome) = check("main.c", "int main(void) { return missing; }\n").await else {
        eprintln!("clangd isn't installed");
        return;
    };
    eprintln!("{}", outcome.render("main.c").unwrap_or_default());
    assert!(has_error(&outcome), "{outcome:?}");
}

#[tokio::test]
#[ignore = "needs typescript-language-server"]
async fn typescript_finds_a_type_error() {
    let Some(outcome) = check("a.ts", "const n: number = \"text\";\n").await else {
        eprintln!("typescript-language-server isn't installed");
        return;
    };
    eprintln!("{}", outcome.render("a.ts").unwrap_or_default());
    assert!(has_error(&outcome), "{outcome:?}");
}

#[tokio::test]
#[ignore = "needs pyright"]
async fn pyright_finds_an_undefined_name() {
    let Some(outcome) = check("a.py", "print(missing)\n").await else {
        eprintln!("pyright isn't installed");
        return;
    };
    eprintln!("{}", outcome.render("a.py").unwrap_or_default());
    assert!(has_error(&outcome), "{outcome:?}");
}
