//! Renews the machine's real SSO token with the refresh grant.
//!
//! The access token IAM Identity Center issues lives an hour; the refresh
//! token beside it lives for the session. This is the difference between an
//! app that asks for a sign-in every hour and one that asks once a day, so
//! the grant is exercised for real rather than mocked. Skipped when no
//! renewable session is cached. Run explicitly: `cargo test --test
//! sso_refresh_live -- --ignored --nocapture`.

use pontifex_lib::aws::{profiles, sso};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

#[test]
#[ignore = "talks to AWS SSO and rewrites the token cache"]
fn a_cached_session_renews_silently() {
    let sessions = profiles::list_sso_sessions().expect("should read ~/.aws/config");
    let Some(session) = sessions.into_iter().find(|s| {
        sso::status_for_key(&s.name)
            .map(|st| st.has_token && st.refreshable)
            .unwrap_or(false)
    }) else {
        eprintln!("skipping: no renewable SSO session cached");
        return;
    };

    let before = sso::status_for_key(&session.name)
        .expect("status")
        .expires_at;
    let until = runtime()
        .block_on(sso::refresh_now(&session.name))
        .expect("refresh grant should succeed for a live session");
    let after = sso::status_for_key(&session.name).expect("status");
    eprintln!("'{}': {:?} -> {until}", session.name, before);
    assert_eq!(after.expires_at.as_deref(), Some(until.as_str()));
    assert!(
        after.refreshable,
        "the renewed cache keeps its refresh material"
    );
    assert!(!after.expired);
}
