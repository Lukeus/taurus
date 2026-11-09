//! The client against a server that speaks the protocol, without needing one
//! installed: see `fake/lsp.rs`.

use std::time::Duration;

use taurus_lsp::{Lsp, Outcome, ServerSpec, Severity};

fn lsp(root: &std::path::Path) -> Lsp {
    let spec = ServerSpec::custom(
        "fake",
        vec![env!("CARGO_BIN_EXE_taurus-fake-lsp").to_string()],
        vec!["fake".into()],
        None,
    );
    Lsp::new(root, vec![spec], Duration::from_secs(10))
}

#[tokio::test]
async fn an_error_in_the_file_comes_back_with_its_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.fake");
    let text = "fine\nBROKEN here\nfine";
    std::fs::write(&path, text).unwrap();
    let lsp = lsp(dir.path());

    let Outcome::Checked {
        server,
        diagnostics,
    } = lsp.check(&path, text).await
    else {
        panic!("expected a check");
    };
    assert_eq!(server, "fake");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].line, 2);
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert_eq!(
        Outcome::Checked {
            server,
            diagnostics
        }
        .render("a.fake")
        .unwrap(),
        "fake reports 1 error in a.fake:\n- 2:1: this line is broken [F1]"
    );
    lsp.shutdown().await;
}

#[tokio::test]
async fn a_second_check_reads_the_new_text_not_the_old_answer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("b.fake");
    std::fs::write(&path, "BROKEN").unwrap();
    let lsp = lsp(dir.path());

    let first = lsp.check(&path, "BROKEN").await;
    assert!(matches!(&first, Outcome::Checked { diagnostics, .. } if diagnostics.len() == 1));

    std::fs::write(&path, "fixed").unwrap();
    let second = lsp.check(&path, "fixed").await;
    assert!(
        matches!(&second, Outcome::Checked { diagnostics, .. } if diagnostics.is_empty()),
        "{second:?}"
    );
    assert_eq!(
        second.render("b.fake").unwrap(),
        "fake: no errors in b.fake."
    );
    lsp.shutdown().await;
}

#[tokio::test]
async fn a_server_that_cannot_start_is_reported_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.fake");
    std::fs::write(&path, "x").unwrap();
    // A program that exists and isn't a language server: it exits at once.
    let program = if cfg!(windows) { "cmd" } else { "true" };
    let spec = ServerSpec::custom(
        "broken",
        if cfg!(windows) {
            vec![program.into(), "/C".into(), "exit".into()]
        } else {
            vec![program.into()]
        },
        vec!["fake".into()],
        None,
    );
    let lsp = Lsp::new(dir.path(), vec![spec], Duration::from_secs(10));

    assert!(matches!(
        lsp.check(&path, "x").await,
        Outcome::Unavailable { .. }
    ));
    assert_eq!(lsp.check(&path, "x").await, Outcome::NotCovered);
}
