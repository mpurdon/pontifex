use crate::aws::clients::{caller_identity, CallerIdentity};
use crate::aws::profiles::{self, AwsProfile};
use crate::aws::sso::{self, SsoStatus};
use crate::aws::sso_credentials;
use crate::error::{Error, Result};
use crate::logging::cat;
use crate::state::AppState;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

/// A profile plus everything the UI needs to decide whether to prompt a login.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileStatus {
    #[serde(flatten)]
    pub profile: AwsProfile,
    pub sso: SsoStatus,
}

#[tauri::command]
pub async fn list_profiles() -> Result<Vec<ProfileStatus>> {
    let profiles = profiles::list_profiles()?;
    let mut out = Vec::with_capacity(profiles.len());
    for profile in profiles {
        let sso = sso::sso_status(&profile)?;
        out.push(ProfileStatus { profile, sso });
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCheck {
    pub profile: String,
    pub region: String,
    pub ok: bool,
    pub identity: Option<CallerIdentity>,
    /// Present when `ok` is false.
    pub error: Option<crate::error::Error>,
    /// True when the failure is specifically an auth problem, i.e. signing in
    /// would likely fix it.
    pub needs_login: bool,
}

/// Turn an identity attempt into a per-profile verdict.
///
/// The one place that decides an outcome is worth offering a sign-in for —
/// spelled out at each call site, `needs_login` had drifted from the error kind
/// it is supposed to track.
fn profile_check(profile: String, region: String, result: Result<CallerIdentity>) -> ProfileCheck {
    match result {
        Ok(identity) => ProfileCheck {
            profile,
            region,
            ok: true,
            identity: Some(identity),
            error: None,
            needs_login: false,
        },
        Err(err) => ProfileCheck {
            needs_login: matches!(err, Error::Auth(_)),
            profile,
            region,
            ok: false,
            identity: None,
            error: Some(err),
        },
    }
}

/// Prove a profile works by calling `sts:GetCallerIdentity`.
///
/// Deliberately returns `Ok` with `ok: false` rather than erroring, so the UI
/// can render a red badge per profile without treating it as a failed request.
#[tauri::command]
pub async fn check_profile(
    state: State<'_, AppState>,
    profile: String,
    region: Option<String>,
) -> Result<ProfileCheck> {
    let region = region.unwrap_or_else(|| crate::settings::DEFAULT_REGION.to_string());
    let cfg = state.clients.config(&profile, &region).await?;
    Ok(profile_check(profile, region, caller_identity(&cfg).await))
}

/// Prove an *environment* works, whichever credential source it uses.
///
/// The header badge checks this rather than a profile, because an environment
/// backed by an in-app SSO target has no profile to check.
#[tauri::command]
pub async fn check_environment(
    state: State<'_, AppState>,
    env_id: String,
) -> Result<ProfileCheck> {
    let env = state.resolve_environment(Some(&env_id)).await?;

    let label = match &env.sso {
        Some(target) => target.label(),
        None => env.aws_profile.clone(),
    };

    let cfg = match state.env_config(Some(&env_id)).await {
        Ok((_, cfg)) => cfg,
        Err(err) => return Ok(profile_check(label, env.region, Err(err))),
    };

    Ok(profile_check(
        label,
        env.region,
        caller_identity(&cfg).await,
    ))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResult {
    pub method: &'static str,
    pub message: String,
    pub expires_at: Option<String>,
}

/// Sign a profile into AWS SSO.
///
/// Runs the native OIDC device-code flow, emitting a `sso://device-code` event
/// so the UI can show the code while the poll runs. Falls back to shelling out
/// to `aws sso login` when the native flow cannot start — for example a
/// profile shape we do not model, or an SSO endpoint that rejects our client
/// registration.
#[tauri::command]
pub async fn sso_login(
    app: AppHandle,
    state: State<'_, AppState>,
    profile: String,
    force_cli: Option<bool>,
) -> Result<LoginResult> {
    let aws_profile = profiles::get_profile(&profile)?;

    if force_cli.unwrap_or(false) {
        let message = sso::login_via_cli(&profile).await?;
        state.clients.invalidate(&profile).await;
        return Ok(LoginResult {
            method: "cli",
            message: if message.is_empty() {
                format!("Signed in as {profile} via the AWS CLI")
            } else {
                message
            },
            expires_at: sso::sso_status(&aws_profile)?.expires_at,
        });
    }

    match sso::begin_login(&aws_profile).await {
        Ok(pending) => {
            // Surface the code immediately; the poll below can take a while.
            let _ = app.emit("sso://device-code", &pending.authorization);

            let url = pending
                .authorization
                .verification_uri_complete
                .clone()
                .unwrap_or_else(|| pending.authorization.verification_uri.clone());
            open_url(&app, &url);

            let token = sso::poll_for_token(pending).await?;
            state.clients.invalidate(&profile).await;

            Ok(LoginResult {
                method: "native",
                message: format!("Signed in as {profile}"),
                expires_at: Some(token.expires_at),
            })
        }
        Err(native_err) => {
            lwarn!(cat::SSO, "native SSO login failed for {profile}, falling back to CLI: {native_err}");
            match sso::login_via_cli(&profile).await {
                Ok(message) => {
                    state.clients.invalidate(&profile).await;
                    Ok(LoginResult {
                        method: "cliFallback",
                        message: if message.is_empty() {
                            format!("Signed in as {profile} via the AWS CLI")
                        } else {
                            message
                        },
                        expires_at: sso::sso_status(&aws_profile)?.expires_at,
                    })
                }
                // Report the *native* failure: it is the more informative of
                // the two, and the CLI failing too usually means the same
                // underlying problem.
                Err(cli_err) => Err(Error::Auth(format!(
                    "Sign-in failed. Built-in: {native_err}. AWS CLI: {cli_err}"
                ))),
            }
        }
    }
}

fn open_url(app: &AppHandle, url: &str) {
    use tauri_plugin_opener::OpenerExt;
    if let Err(e) = app.opener().open_url(url, None::<&str>) {
        lwarn!(cat::SSO, "could not open browser for SSO verification: {e}");
    }
}

/// Drop every cached SDK config. Useful after editing ~/.aws/config by hand.
#[tauri::command]
pub async fn refresh_credentials(state: State<'_, AppState>) -> Result<()> {
    state.clients.invalidate_all().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// SSO sessions — signing in once, then picking accounts and roles from it
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SsoSessionStatus {
    #[serde(flatten)]
    pub session: crate::aws::profiles::SsoSession,
    pub sso: SsoStatus,
}

/// Every `[sso-session]` in ~/.aws/config, with its sign-in state.
#[tauri::command]
pub async fn list_sso_sessions() -> Result<Vec<SsoSessionStatus>> {
    let sessions = profiles::list_sso_sessions()?;
    let mut out = Vec::with_capacity(sessions.len());
    for session in sessions {
        let sso = sso::status_for_key(&session.name)?;
        out.push(SsoSessionStatus { session, sso });
    }
    Ok(out)
}

/// Sign in to an SSO session directly, without a profile.
///
/// One sign-in covers every account and role the session grants, which is what
/// lets environments point at accounts that have no profile of their own.
#[tauri::command]
pub async fn sso_session_login(
    app: AppHandle,
    state: State<'_, AppState>,
    session: String,
) -> Result<LoginResult> {
    let info = profiles::get_sso_session(&session)?;

    let pending = sso::begin_login_with(&info.start_url, &info.sso_region, &info.name).await?;

    let _ = app.emit("sso://device-code", &pending.authorization);
    let url = pending
        .authorization
        .verification_uri_complete
        .clone()
        .unwrap_or_else(|| pending.authorization.verification_uri.clone());
    open_url(&app, &url);

    let token = sso::poll_for_token(pending).await?;
    state.clients.invalidate_session(&session).await;

    Ok(LoginResult {
        method: "native",
        message: format!("Signed in to '{session}'"),
        expires_at: Some(token.expires_at),
    })
}

/// Accounts the signed-in session grants access to.
#[tauri::command]
pub async fn list_sso_accounts(session: String) -> Result<Vec<sso_credentials::SsoAccount>> {
    sso_credentials::list_accounts(&session).await
}

/// Roles the session grants in a given account.
#[tauri::command]
pub async fn list_sso_account_roles(session: String, account_id: String) -> Result<Vec<String>> {
    sso_credentials::list_account_roles(&session, &account_id).await
}
