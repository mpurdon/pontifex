//! Verifies error classification against a *real* AWS SDK failure rather than a
//! hand-written string.
//!
//! The message the SDK produces for "this profile has never been signed in"
//! lives four levels deep in the error's source chain and is not visible from
//! its Display. Classifying it as an auth problem is what makes the UI offer a
//! Sign in button instead of a dead-end error, so it is worth pinning to the
//! real thing.
//!
//! Skipped when no such profile exists locally.

use gebman_lib::aws::clients::caller_identity;
use gebman_lib::error::Error;

/// A profile that resolves to an SSO token cache file which does not exist.
/// Any profile that has never been logged into produces this.
fn profile_without_a_token() -> Option<String> {
    let profiles = gebman_lib::aws::profiles::list_profiles().ok()?;
    profiles
        .into_iter()
        .filter(|p| p.is_sso())
        .find(|p| {
            matches!(
                gebman_lib::aws::sso::sso_status(p),
                Ok(status) if !status.has_token
            )
        })
        .map(|p| p.name)
}

#[test]
fn missing_sso_token_is_classified_as_an_auth_error() {
    let Some(profile) = profile_without_a_token() else {
        eprintln!("skipping: every SSO profile on this machine has a cached token");
        return;
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let result = rt.block_on(async {
        let cfg = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .profile_name(&profile)
            .region(aws_config::Region::new("us-east-1".to_string()))
            .load()
            .await;
        caller_identity(&cfg).await
    });

    let err = match result {
        Ok(identity) => {
            eprintln!("skipping: '{profile}' unexpectedly authenticated as {identity:?}");
            return;
        }
        Err(e) => e,
    };

    assert!(
        matches!(err, Error::Auth(_)),
        "a missing SSO token must classify as Auth so the UI offers a sign-in, got: {err:?}"
    );
}
