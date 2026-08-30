//! Project-scoped Docker Compose inspection and lifecycle actions.
//!
//! The API deliberately exposes a closed action enum rather than an arbitrary
//! command string. A caller may target the complete Compose project or one
//! service returned by `docker compose config --services`; service names are
//! validated before they ever become process arguments.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    path::{Path as FsPath, PathBuf},
    process::Output,
    time::Duration,
};

use crate::{
    core::{cmd::async_cmd, scanner},
    models::{
        ApiResponse, ProjectDockerAction, ProjectDockerActionRequest, ProjectDockerEndpoint,
        ProjectDockerHostStatus, ProjectDockerLogs, ProjectDockerRunningSummary,
        ProjectDockerService, ProjectDockerStatus,
    },
    AppState,
};

const COMPOSE_FILES: [&str; 4] = [
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
];
const STATUS_TIMEOUT: Duration = Duration::from_secs(15);
const ACTION_TIMEOUT: Duration = Duration::from_secs(180);
const LOGS_TIMEOUT: Duration = Duration::from_secs(20);
const COMPOSE_WORKING_DIR_LABEL: &str = "com.docker.compose.project.working_dir";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfiguredDockerService {
    name: String,
    endpoints: Vec<ProjectDockerEndpoint>,
}

type HostMappings = HashMap<String, Vec<IpAddr>>;

#[derive(Debug, Deserialize)]
pub struct ProjectDockerLogsQuery {
    service: String,
    tail: Option<u16>,
}

async fn resolve_project_root(state: &AppState, id: &str) -> Result<PathBuf, String> {
    let project_id = id.to_string();
    let project = state
        .db
        .with_read_conn(move |conn| crate::db::projects::get_project(conn, &project_id))
        .await
        .map_err(|error| format!("DB error: {error}"))?
        .ok_or_else(|| "Project not found".to_string())?;

    // Docker receives absolute bind paths through the host daemon. Prefer the
    // original host path when Kronn's Docker deployment self-mounted it at the
    // same location; `/host-home/...` remains the fallback for older setups.
    let stored_path = PathBuf::from(&project.path);
    let root = if stored_path.is_dir() {
        stored_path
    } else {
        scanner::resolve_host_path(&project.path)
    };
    if !root.is_dir() {
        return Err(format!("Project path not found: {}", root.display()));
    }
    Ok(root)
}

fn find_compose_file(root: &FsPath) -> Option<String> {
    COMPOSE_FILES
        .iter()
        .find(|name| root.join(name).is_file())
        .map(|name| (*name).to_string())
}

fn docker_command(root: &FsPath, compose_file: &str, args: &[&str]) -> tokio::process::Command {
    let mut command = async_cmd("docker");
    command
        .args(["compose", "--ansi", "never", "-f", compose_file])
        .args(args)
        .current_dir(root)
        .env("CI", "1")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .kill_on_drop(true);
    command
}

async fn capture_command(
    mut command: tokio::process::Command,
    timeout: Duration,
) -> Result<Output, String> {
    match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(format!("Docker Compose is unavailable: {error}")),
        Err(_) => Err("Docker Compose command timed out".to_string()),
    }
}

fn bounded_command_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    if detail.is_empty() {
        return format!("Docker Compose exited with {}", output.status);
    }
    detail.chars().take(800).collect()
}

fn hosts_file_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("KRONN_HOSTS_FILE") {
        return Some(PathBuf::from(path));
    }

    if FsPath::new("/.dockerenv").exists() {
        let host_file = PathBuf::from("/host-etc/hosts");
        return host_file.is_file().then_some(host_file);
    }

    #[cfg(windows)]
    {
        std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .map(|root| root.join("System32/drivers/etc/hosts"))
    }
    #[cfg(not(windows))]
    {
        Some(PathBuf::from("/etc/hosts"))
    }
}

fn parse_host_mappings(contents: &str) -> HostMappings {
    let mut mappings = HostMappings::new();
    for line in contents.lines() {
        let mut fields = line
            .split('#')
            .next()
            .unwrap_or_default()
            .split_whitespace();
        let Some(address) = fields.next().and_then(|value| value.parse::<IpAddr>().ok()) else {
            continue;
        };
        for hostname in fields {
            mappings
                .entry(hostname.trim_end_matches('.').to_ascii_lowercase())
                .or_default()
                .push(address);
        }
    }
    mappings
}

fn read_host_mappings() -> Option<HostMappings> {
    let path = hosts_file_path()?;
    let contents = std::fs::read_to_string(path).ok()?;
    if contents.contains("KRONN_HOSTS_UNAVAILABLE") {
        return None;
    }
    Some(parse_host_mappings(&contents))
}

fn host_status(host: &str, mappings: Option<&HostMappings>) -> ProjectDockerHostStatus {
    let normalized = host
        .trim_matches(['[', ']'])
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if normalized == "localhost" {
        return ProjectDockerHostStatus::Configured;
    }
    if let Ok(address) = normalized.parse::<IpAddr>() {
        return if address.is_loopback() {
            ProjectDockerHostStatus::Configured
        } else {
            ProjectDockerHostStatus::NonLocal
        };
    }
    let Some(mappings) = mappings else {
        return ProjectDockerHostStatus::Unknown;
    };
    match mappings.get(&normalized) {
        Some(addresses) if addresses.iter().any(IpAddr::is_loopback) => {
            ProjectDockerHostStatus::Configured
        }
        Some(_) => ProjectDockerHostStatus::NonLocal,
        None => ProjectDockerHostStatus::Missing,
    }
}

fn value_port(value: Option<&Value>) -> Option<u16> {
    value.and_then(|value| {
        value
            .as_u64()
            .and_then(|port| u16::try_from(port).ok())
            .or_else(|| value.as_str()?.parse::<u16>().ok())
    })
}

fn service_ports(service: &Value) -> Vec<(u16, u16)> {
    service
        .get("ports")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|port| {
            let target = value_port(port.get("target"))?;
            let published = value_port(port.get("published")).unwrap_or(target);
            Some((target, published))
        })
        .collect()
}

fn service_environment(service: &Value) -> HashMap<String, String> {
    let Some(environment) = service.get("environment") else {
        return HashMap::new();
    };
    if let Some(values) = environment.as_object() {
        return values
            .iter()
            .filter_map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.to_ascii_uppercase(), value.to_string()))
            })
            .collect();
    }
    environment
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|entry| entry.split_once('='))
        .map(|(key, value)| (key.to_ascii_uppercase(), value.to_string()))
        .collect()
}

fn service_labels(service: &Value) -> HashMap<String, String> {
    let Some(labels) = service.get("labels") else {
        return HashMap::new();
    };
    if let Some(values) = labels.as_object() {
        return values
            .iter()
            .filter_map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.to_ascii_lowercase(), value.to_string()))
            })
            .collect();
    }
    labels
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|entry| entry.split_once('='))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.to_string()))
        .collect()
}

fn normalize_host(value: &str) -> Option<String> {
    let host = value.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty()
        || host.starts_with("*.")
        || host
            .chars()
            .any(|character| ['/', '$', '@'].contains(&character))
        || !host
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-:[]".contains(character))
    {
        return None;
    }
    Some(host)
}

fn url_host(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split('/').next()?.split('?').next()?;
    if authority.contains('@') {
        return None;
    }
    if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, _) = bracketed.split_once(']')?;
        return Some(host.to_ascii_lowercase());
    }
    let host = authority
        .rsplit_once(':')
        .filter(|(_, port)| port.parse::<u16>().is_ok())
        .map_or(authority, |(host, _)| host);
    normalize_host(host)
}

fn normalize_url(value: &str) -> Option<String> {
    let url = value.trim().trim_end_matches('/');
    if !(url.starts_with("http://") || url.starts_with("https://"))
        || url.chars().any(char::is_whitespace)
        || url_host(url).is_none()
    {
        return None;
    }
    Some(url.to_string())
}

fn hosts_from_rule(rule: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    for quote in ['`', '\'', '"'] {
        let mut chunks = rule.split(quote);
        while let (Some(_), Some(value)) = (chunks.next(), chunks.next()) {
            if let Some(host) = normalize_host(value) {
                hosts.push(host);
            }
        }
    }
    hosts
}

fn preferred_web_port(ports: &[(u16, u16)]) -> Option<(u16, u16)> {
    const WEB_PORTS: [u16; 10] = [443, 80, 3000, 4173, 4200, 5173, 8000, 8080, 8081, 8888];
    WEB_PORTS
        .iter()
        .find_map(|target| ports.iter().find(|(port, _)| port == target).copied())
}

fn host_url(host: &str, port: Option<(u16, u16)>, tls_hint: bool) -> String {
    let (scheme, published) = match port {
        Some((443, published)) => ("https", Some(published)),
        Some((80, published)) => ("http", Some(published)),
        Some((_, published)) => (if tls_hint { "https" } else { "http" }, Some(published)),
        None => (if tls_hint { "https" } else { "http" }, None),
    };
    let default_port = (scheme == "https" && published == Some(443))
        || (scheme == "http" && published == Some(80));
    match published.filter(|_| !default_port) {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    }
}

fn service_endpoints(
    service: &Value,
    mappings: Option<&HostMappings>,
) -> Vec<ProjectDockerEndpoint> {
    let environment = service_environment(service);
    let labels = service_labels(service);
    let ports = service_ports(service);
    let web_port = preferred_web_port(&ports);
    let tls_hint = web_port.is_some_and(|(target, _)| target == 443)
        || labels.iter().any(|(key, value)| {
            (key.contains(".tls") && value.eq_ignore_ascii_case("true"))
                || (key.ends_with(".entrypoints") && value.contains("websecure"))
        });
    let mut urls = Vec::new();

    for key in ["APP_URL", "SITE_URL", "PUBLIC_URL", "BASE_URL"] {
        if let Some(url) = environment.get(key).and_then(|value| normalize_url(value)) {
            urls.push(url);
        }
    }

    let mut hosts = Vec::new();
    for key in ["SERVER_NAME", "VIRTUAL_HOST", "VIRTUAL_HOSTS"] {
        if let Some(value) = environment.get(key) {
            hosts.extend(
                value
                    .split(|character: char| character.is_whitespace() || character == ',')
                    .filter_map(normalize_host),
            );
        }
    }
    for (key, value) in &labels {
        if key == "caddy" || key.ends_with(".virtual_host") || key.ends_with(".virtual_hosts") {
            hosts.extend(
                value
                    .split(|character: char| character.is_whitespace() || character == ',')
                    .filter_map(normalize_host),
            );
        }
        if key.starts_with("traefik.http.routers.") && key.ends_with(".rule") {
            hosts.extend(hosts_from_rule(value));
        }
    }
    hosts.sort();
    hosts.dedup();
    urls.extend(
        hosts
            .into_iter()
            .map(|host| host_url(&host, web_port, tls_hint)),
    );

    if urls.is_empty() {
        if let Some(port) = web_port {
            urls.push(host_url("localhost", Some(port), false));
        }
    }

    let mut seen = HashSet::new();
    urls.into_iter()
        .filter_map(|url| {
            let host = url_host(&url)?;
            seen.insert(url.clone()).then(|| ProjectDockerEndpoint {
                host_status: host_status(&host, mappings),
                host,
                url,
            })
        })
        .collect()
}

fn parse_compose_config(
    output: &str,
    mappings: Option<&HostMappings>,
) -> Result<Vec<ConfiguredDockerService>, String> {
    let config: Value = serde_json::from_str(output)
        .map_err(|error| format!("Invalid Docker Compose configuration: {error}"))?;
    let services = config
        .get("services")
        .and_then(Value::as_object)
        .ok_or_else(|| "Docker Compose configuration has no services".to_string())?;
    let mut configured = services
        .iter()
        .map(|(name, service)| ConfiguredDockerService {
            name: name.clone(),
            endpoints: service_endpoints(service, mappings),
        })
        .collect::<Vec<_>>();
    configured.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(configured)
}

fn value_string(value: &Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn publisher_port(value: &Value) -> Option<String> {
    let target = value
        .get("TargetPort")
        .or_else(|| value.get("target_port"))
        .and_then(Value::as_u64)?;
    let published = value
        .get("PublishedPort")
        .or_else(|| value.get("published_port"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let protocol =
        value_string(value, &["Protocol", "protocol"]).unwrap_or_else(|| "tcp".to_string());
    let url = value_string(value, &["URL", "url"]);
    Some(if published > 0 {
        format!(
            "{}{} → {target}/{protocol}",
            url.map(|host| format!("{host}:")).unwrap_or_default(),
            published
        )
    } else {
        format!("{target}/{protocol}")
    })
}

fn parse_ps_values(output: &str) -> Result<Vec<Value>, String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        return serde_json::from_str(trimmed)
            .map_err(|error| format!("Invalid Docker Compose status: {error}"));
    }
    trimmed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .map_err(|error| format!("Invalid Docker Compose status: {error}"))
        })
        .collect()
}

fn parse_services(
    output: &str,
    configured: &[ConfiguredDockerService],
) -> Result<Vec<ProjectDockerService>, String> {
    let values = parse_ps_values(output)?;
    let mut services = values
        .iter()
        .filter_map(|value| {
            let service = value_string(value, &["Service", "service"])?;
            let state =
                value_string(value, &["State", "state"]).unwrap_or_else(|| "unknown".to_string());
            let ports = value
                .get("Publishers")
                .or_else(|| value.get("publishers"))
                .and_then(Value::as_array)
                .map(|publishers| publishers.iter().filter_map(publisher_port).collect())
                .unwrap_or_default();
            Some(ProjectDockerService {
                endpoints: configured
                    .iter()
                    .find(|entry| entry.name == service)
                    .map(|entry| entry.endpoints.clone())
                    .unwrap_or_default(),
                service,
                container_name: value_string(value, &["Name", "name"]),
                image: value_string(value, &["Image", "image"]),
                running: state.eq_ignore_ascii_case("running"),
                state,
                status: value_string(value, &["Status", "status"]),
                health: value_string(value, &["Health", "health"]),
                ports,
            })
        })
        .collect::<Vec<_>>();

    for service in configured {
        if !services.iter().any(|entry| entry.service == service.name) {
            services.push(ProjectDockerService {
                service: service.name.clone(),
                container_name: None,
                image: None,
                state: "not_created".to_string(),
                status: None,
                health: None,
                ports: Vec::new(),
                endpoints: service.endpoints.clone(),
                running: false,
            });
        }
    }
    services.sort_by(|left, right| {
        left.service
            .cmp(&right.service)
            .then(left.container_name.cmp(&right.container_name))
    });
    Ok(services)
}

async fn configured_services(
    root: &FsPath,
    compose_file: &str,
) -> Result<Vec<ConfiguredDockerService>, String> {
    let mappings = read_host_mappings();
    let metadata = capture_command(
        docker_command(root, compose_file, &["config", "--format", "json"]),
        STATUS_TIMEOUT,
    )
    .await?;
    if metadata.status.success() {
        if let Ok(services) = parse_compose_config(
            &String::from_utf8_lossy(&metadata.stdout),
            mappings.as_ref(),
        ) {
            return Ok(services);
        }
    }

    // Older Compose releases do not expose the JSON config format. Keep the
    // service controls available, but omit endpoint metadata in that case.
    let output = capture_command(
        docker_command(root, compose_file, &["config", "--services"]),
        STATUS_TIMEOUT,
    )
    .await?;
    if !output.status.success() {
        return Err(bounded_command_error(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|service| !service.is_empty())
        .map(|service| ConfiguredDockerService {
            name: service.to_string(),
            endpoints: Vec::new(),
        })
        .collect())
}

fn empty_status() -> ProjectDockerStatus {
    ProjectDockerStatus {
        compose_present: false,
        compose_file: None,
        docker_available: false,
        daemon_available: false,
        services: Vec::new(),
        checked_at: Utc::now(),
        error: None,
    }
}

async fn inspect_compose(root: &FsPath, compose_file: &str) -> ProjectDockerStatus {
    let mut status = ProjectDockerStatus {
        compose_present: true,
        compose_file: Some(compose_file.to_string()),
        docker_available: false,
        daemon_available: false,
        services: Vec::new(),
        checked_at: Utc::now(),
        error: None,
    };
    let configured = match configured_services(root, compose_file).await {
        Ok(services) => {
            status.docker_available = true;
            services
        }
        Err(error) => {
            status.error = Some(error);
            return status;
        }
    };

    let output = match capture_command(
        docker_command(root, compose_file, &["ps", "-a", "--format", "json"]),
        STATUS_TIMEOUT,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => {
            status.error = Some(error);
            return status;
        }
    };
    if !output.status.success() {
        status.error = Some(bounded_command_error(&output));
        return status;
    }
    status.daemon_available = true;
    match parse_services(&String::from_utf8_lossy(&output.stdout), &configured) {
        Ok(services) => status.services = services,
        Err(error) => status.error = Some(error),
    }
    status
}

fn action_args(action: ProjectDockerAction, service: Option<&str>) -> Vec<String> {
    let mut args = match action {
        ProjectDockerAction::Start => vec!["up".to_string(), "-d".to_string()],
        ProjectDockerAction::Stop => vec!["stop".to_string()],
        ProjectDockerAction::Restart => vec!["restart".to_string()],
    };
    if let Some(service) = service {
        args.push(service.to_string());
    }
    args
}

fn logs_args(service: &str, tail: u16) -> Vec<String> {
    vec![
        "logs".to_string(),
        "--no-color".to_string(),
        "--timestamps".to_string(),
        "--tail".to_string(),
        tail.to_string(),
        service.to_string(),
    ]
}

fn comparable_path(path: &FsPath) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn match_running_project_ids(
    projects: &[(String, PathBuf)],
    working_directories: &str,
) -> Vec<String> {
    let running = working_directories
        .lines()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .map(|path| comparable_path(&path))
        .collect::<HashSet<_>>();
    let mut project_ids = projects
        .iter()
        .filter(|(_, path)| running.contains(&comparable_path(path)))
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    project_ids.sort();
    project_ids.dedup();
    project_ids
}

/// GET /api/projects/docker-running
pub async fn docker_running_projects(
    State(state): State<AppState>,
) -> Json<ApiResponse<ProjectDockerRunningSummary>> {
    let mut command = async_cmd("docker");
    let label_filter = format!("label={COMPOSE_WORKING_DIR_LABEL}");
    let label_template = format!("{{{{.Label \"{COMPOSE_WORKING_DIR_LABEL}\"}}}}");
    command
        .args([
            "ps",
            "--filter",
            "status=running",
            "--filter",
            &label_filter,
            "--format",
            &label_template,
        ])
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .kill_on_drop(true);
    let output = match capture_command(command, STATUS_TIMEOUT).await {
        Ok(output) => output,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    if !output.status.success() {
        return Json(ApiResponse::err(bounded_command_error(&output)));
    }

    let projects = match state.db.with_conn(crate::db::projects::list_projects).await {
        Ok(projects) => projects
            .into_iter()
            .map(|project| {
                let stored = PathBuf::from(&project.path);
                let path = if stored.is_dir() {
                    stored
                } else {
                    scanner::resolve_host_path(&project.path)
                };
                (project.id, path)
            })
            .collect::<Vec<_>>(),
        Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
    };
    Json(ApiResponse::ok(ProjectDockerRunningSummary {
        project_ids: match_running_project_ids(&projects, &String::from_utf8_lossy(&output.stdout)),
        checked_at: Utc::now(),
    }))
}

/// GET /api/projects/{id}/docker
pub async fn docker_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<ProjectDockerStatus>> {
    let root = match resolve_project_root(&state, &id).await {
        Ok(root) => root,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let Some(compose_file) = find_compose_file(&root) else {
        return Json(ApiResponse::ok(empty_status()));
    };
    Json(ApiResponse::ok(inspect_compose(&root, &compose_file).await))
}

/// GET /api/projects/{id}/docker/logs
pub async fn docker_logs(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ProjectDockerLogsQuery>,
) -> Json<ApiResponse<ProjectDockerLogs>> {
    let root = match resolve_project_root(&state, &id).await {
        Ok(root) => root,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let Some(compose_file) = find_compose_file(&root) else {
        return Json(ApiResponse::err(
            "No Docker Compose file found at project root",
        ));
    };
    let configured = match configured_services(&root, &compose_file).await {
        Ok(services) => services,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let service = query.service.trim();
    if service.is_empty()
        || !configured
            .iter()
            .any(|configured| configured.name == service)
    {
        return Json(ApiResponse::err(format!(
            "Unknown Docker Compose service: {service}"
        )));
    }

    let args = logs_args(service, query.tail.unwrap_or(200).clamp(20, 1_000));
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = match capture_command(
        docker_command(&root, &compose_file, &arg_refs),
        LOGS_TIMEOUT,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    if !output.status.success() {
        return Json(ApiResponse::err(bounded_command_error(&output)));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (false, false) => format!("{stdout}\n{stderr}"),
        (false, true) => stdout.into_owned(),
        (true, false) => stderr.into_owned(),
        (true, true) => String::new(),
    };
    // Keep the response bounded even when a Compose implementation ignores
    // `--tail`; retain the most recent output because that is what users need.
    let output = if combined.len() > 512_000 {
        let target = combined.len() - 512_000;
        let start = (target..combined.len())
            .find(|index| combined.is_char_boundary(*index))
            .unwrap_or(target);
        combined[start..].to_string()
    } else {
        combined
    };
    Json(ApiResponse::ok(ProjectDockerLogs {
        service: service.to_string(),
        output,
        fetched_at: Utc::now(),
    }))
}

/// POST /api/projects/{id}/docker
pub async fn docker_action(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ProjectDockerActionRequest>,
) -> Json<ApiResponse<ProjectDockerStatus>> {
    let root = match resolve_project_root(&state, &id).await {
        Ok(root) => root,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let Some(compose_file) = find_compose_file(&root) else {
        return Json(ApiResponse::err(
            "No Docker Compose file found at project root",
        ));
    };
    let configured = match configured_services(&root, &compose_file).await {
        Ok(services) => services,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let requested_service = request
        .service
        .as_deref()
        .map(str::trim)
        .filter(|service| !service.is_empty());
    if let Some(service) = requested_service {
        if !configured
            .iter()
            .any(|configured| configured.name == service)
        {
            return Json(ApiResponse::err(format!(
                "Unknown Docker Compose service: {service}"
            )));
        }
    }

    let args = action_args(request.action, requested_service);
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = match capture_command(
        docker_command(&root, &compose_file, &arg_refs),
        ACTION_TIMEOUT,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    if !output.status.success() {
        return Json(ApiResponse::err(bounded_command_error(&output)));
    }
    Json(ApiResponse::ok(inspect_compose(&root, &compose_file).await))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(name: &str) -> ConfiguredDockerService {
        ConfiguredDockerService {
            name: name.to_string(),
            endpoints: Vec::new(),
        }
    }

    #[test]
    fn compose_detection_uses_docker_precedence_and_root_only() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("docker-compose.yml"),
            "services: {}\n",
        )
        .unwrap();
        std::fs::write(directory.path().join("compose.yaml"), "services: {}\n").unwrap();
        std::fs::create_dir(directory.path().join("nested")).unwrap();
        std::fs::write(
            directory.path().join("nested/compose.yml"),
            "services: {}\n",
        )
        .unwrap();

        assert_eq!(
            find_compose_file(directory.path()).as_deref(),
            Some("compose.yaml")
        );
    }

    #[test]
    fn parses_array_status_and_keeps_services_without_containers() {
        let output = r#"[{"Service":"web","Name":"demo-web-1","Image":"nginx:alpine","State":"running","Status":"Up 2 minutes","Health":"healthy","Publishers":[{"URL":"0.0.0.0","TargetPort":80,"PublishedPort":8080,"Protocol":"tcp"}]}]"#;
        let services = parse_services(output, &[configured("web"), configured("worker")]).unwrap();

        assert_eq!(services.len(), 2);
        assert_eq!(services[0].service, "web");
        assert!(services[0].running);
        assert_eq!(services[0].ports, vec!["0.0.0.0:8080 → 80/tcp"]);
        assert_eq!(services[1].service, "worker");
        assert_eq!(services[1].state, "not_created");
        assert!(!services[1].running);
    }

    #[test]
    fn parses_line_delimited_compose_status() {
        let output = "{\"Service\":\"api\",\"Name\":\"demo-api-1\",\"State\":\"exited\"}\n{\"Service\":\"db\",\"Name\":\"demo-db-1\",\"State\":\"running\"}\n";
        let services = parse_services(output, &[configured("api"), configured("db")]).unwrap();
        assert_eq!(services.len(), 2);
        assert!(!services[0].running);
        assert!(services[1].running);
    }

    #[test]
    fn action_arguments_are_closed_and_service_is_a_single_argument() {
        assert_eq!(
            action_args(ProjectDockerAction::Start, Some("web")),
            vec!["up", "-d", "web"]
        );
        assert_eq!(action_args(ProjectDockerAction::Stop, None), vec!["stop"]);
        assert_eq!(
            action_args(ProjectDockerAction::Restart, Some("worker")),
            vec!["restart", "worker"]
        );
    }

    #[test]
    fn log_arguments_are_bounded_and_keep_service_as_one_argument() {
        assert_eq!(
            logs_args("web app", 200),
            vec![
                "logs",
                "--no-color",
                "--timestamps",
                "--tail",
                "200",
                "web app",
            ]
        );
    }

    #[test]
    fn matches_running_compose_directories_to_registered_projects_once() {
        let running = tempfile::tempdir().unwrap();
        let stopped = tempfile::tempdir().unwrap();
        let projects = vec![
            ("running-project".to_string(), running.path().to_path_buf()),
            ("stopped-project".to_string(), stopped.path().to_path_buf()),
        ];
        let output = format!(
            "{}\n{}\n",
            running.path().display(),
            running.path().display()
        );

        assert_eq!(
            match_running_project_ids(&projects, &output),
            vec!["running-project"]
        );
    }

    #[test]
    fn parses_hosts_file_and_reports_local_mapping_state() {
        let mappings = parse_host_mappings(
            "127.0.0.1 app.local alias.local # project\n192.0.2.10 remote.local\n::1 ipv6.local\n",
        );

        assert_eq!(
            host_status("app.local", Some(&mappings)),
            ProjectDockerHostStatus::Configured
        );
        assert_eq!(
            host_status("remote.local", Some(&mappings)),
            ProjectDockerHostStatus::NonLocal
        );
        assert_eq!(
            host_status("missing.local", Some(&mappings)),
            ProjectDockerHostStatus::Missing
        );
        assert_eq!(
            host_status("missing.local", None),
            ProjectDockerHostStatus::Unknown
        );
    }

    #[test]
    fn derives_https_hosts_and_local_web_ports_from_compose_config() {
        let mappings = parse_host_mappings("127.0.0.1 www.example.local\n");
        let config = r#"{
          "services": {
            "php": {
              "environment": {"SERVER_NAME": "www.example.local fr.example.local"},
              "ports": [{"target": 443, "published": "443"}, {"target": 9000, "published": "9000"}]
            },
            "swagger": {
              "ports": [{"target": 8080, "published": "3615"}]
            }
          }
        }"#;

        let services = parse_compose_config(config, Some(&mappings)).unwrap();
        let php = services
            .iter()
            .find(|service| service.name == "php")
            .unwrap();
        assert_eq!(php.endpoints.len(), 2);
        assert_eq!(php.endpoints[0].url, "https://fr.example.local");
        assert_eq!(
            php.endpoints[0].host_status,
            ProjectDockerHostStatus::Missing
        );
        assert_eq!(php.endpoints[1].url, "https://www.example.local");
        assert_eq!(
            php.endpoints[1].host_status,
            ProjectDockerHostStatus::Configured
        );

        let swagger = services
            .iter()
            .find(|service| service.name == "swagger")
            .unwrap();
        assert_eq!(swagger.endpoints[0].url, "http://localhost:3615");
        assert_eq!(
            swagger.endpoints[0].host_status,
            ProjectDockerHostStatus::Configured
        );
    }

    #[test]
    fn derives_hosts_from_reverse_proxy_labels_and_explicit_urls() {
        let config = r#"{
          "services": {
            "app": {
              "environment": {"PUBLIC_URL": "https://app.example.test/"},
              "labels": {
                "caddy": "caddy.example.test",
                "traefik.http.routers.app.rule": "Host(`app.example.test`) || Host(`admin.example.test`)",
                "traefik.http.routers.app.tls": "true"
              }
            }
          }
        }"#;

        let services = parse_compose_config(config, None).unwrap();
        assert_eq!(
            services[0]
                .endpoints
                .iter()
                .map(|endpoint| endpoint.url.as_str())
                .collect::<Vec<_>>(),
            vec![
                "https://app.example.test",
                "https://admin.example.test",
                "https://caddy.example.test",
            ]
        );
        assert!(services[0]
            .endpoints
            .iter()
            .all(|endpoint| endpoint.host_status == ProjectDockerHostStatus::Unknown));
    }
}
