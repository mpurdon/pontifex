//! Exercises SSO session discovery against the machine's real `~/.aws/config`.
//!
//! These paths are what let pontifex reach an account with no profile behind it,
//! so they need to behave sanely in the state users actually hit: a session
//! that is declared but whose token has lapsed.
//!
//! Skipped when no SSO session is configured locally.

use pontifex_lib::aws::{profiles, sso, sso_credentials};
use pontifex_lib::error::Error;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

#[test]
fn discovers_sso_sessions_from_config() {
    let sessions = profiles::list_sso_sessions().expect("should read ~/.aws/config");
    if sessions.is_empty() {
        eprintln!("skipping: no [sso-session] blocks configured");
        return;
    }

    for session in &sessions {
        assert!(!session.name.is_empty());
        // A session we would offer a sign-in for must be actionable.
        assert!(
            session.start_url.starts_with("http"),
            "session '{}' has an unusable start URL: {}",
            session.name,
            session.start_url
        );
        assert!(!session.sso_region.is_empty(), "session '{}'", session.name);
    }

    // Every session must resolve by name, since that is how environments
    // reference them.
    for session in &sessions {
        profiles::get_sso_session(&session.name).expect("session should resolve by name");
    }
}

#[test]
fn unknown_session_is_a_not_found_error() {
    let err = profiles::get_sso_session("definitely-not-a-real-session").unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "got: {err:?}");
}

#[test]
fn expired_or_missing_token_surfaces_as_auth_not_a_crash() {
    let Some(session) = profiles::list_sso_sessions().ok().and_then(|s| s.into_iter().next())
    else {
        eprintln!("skipping: no [sso-session] blocks configured");
        return;
    };

    let status = sso::status_for_key(&session.name).expect("status should read");
    if status.has_token && !status.expired {
        eprintln!("skipping: '{}' currently has a live token", session.name);
        return;
    }

    // Discovery against a lapsed session must route the user to a sign-in
    // rather than failing as an opaque API error.
    let result = runtime().block_on(sso_credentials::list_accounts(&session.name));
    match result {
        Ok(accounts) => panic!("expected an auth error, got {} accounts", accounts.len()),
        Err(err) => assert!(
            matches!(err, Error::Auth(_)),
            "a lapsed session must classify as Auth so the UI offers a sign-in, got: {err:?}"
        ),
    }
}
