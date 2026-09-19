//! The origin lookup against the real organisation, with the GitHub CLI's
//! login. Skipped when no token is available. Run explicitly:
//! `cargo test --test origin_live -- --ignored --nocapture`.

use pontifex_lib::origin::{github, wiring};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

#[test]
#[ignore = "talks to GitHub"]
fn finds_who_first_published_an_event_type() {
    let rt = runtime();
    let (token, source) = rt.block_on(github::token()).expect("token lookup");
    let Some(token) = token else {
        eprintln!("skipping: no GitHub token");
        return;
    };
    eprintln!("token via {source:?}");
    let client = github::Client::new(token);
    let started = std::time::Instant::now();
    let (producers, outcome) = rt.block_on(github::lookup(
        &client,
        "team-and-tech",
        "clientProfile-medical",
        "clientProfile-created",
        Some("team-and-tech/global-event-bus"),
        &pontifex_lib::default_origin_ignore(),
        github::MAX_FILES,
    ));
    eprintln!("outcome {outcome:?} in {:?}", started.elapsed());
    for p in &producers {
        eprintln!(
            "{} {} role={:?} incidental={} owners={:?} reads={:?}\n    {}",
            p.repo,
            p.path,
            p.role,
            p.incidental,
            p.owners,
            p.reads,
            p.introduced
                .as_ref()
                .map(|i| format!(
                    "first by {} on {} — {} (PR {:?} {:?})",
                    i.author, i.date, i.subject, i.pull_number, i.pull_title
                ))
                .unwrap_or_else(|| "introducing commit not found".into())
        );
    }
    assert_eq!(outcome.status, "ok");
    assert!(
        !producers.is_empty(),
        "expected producers for a live event type"
    );

    let repo = std::path::PathBuf::from(
        std::env::var("HOME").unwrap() + "/Projects/trajector/global-event-bus",
    );
    let wired = rt
        .block_on(wiring::first_mention(&repo, "clientProfile-created"))
        .unwrap();
    eprintln!("wiring: {wired:?}");
}
