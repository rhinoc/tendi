use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{sessions::SessionRecord, skills::AgentKind, time::parse_timestamp};

const ANALYTICS_PARSER_VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsTokenUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
}

impl AnalyticsTokenUsage {
    pub fn add_assign(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.cache_write_input_tokens = self
            .cache_write_input_tokens
            .saturating_add(other.cache_write_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_output_tokens = self
            .reasoning_output_tokens
            .saturating_add(other.reasoning_output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsCapabilities {
    pub token_usage: bool,
    pub reasoning_tokens: bool,
    pub explicit_runs: bool,
    pub rate_limit_history: bool,
}

impl AnalyticsCapabilities {
    pub fn for_agent(agent: AgentKind) -> Self {
        crate::providers::agent_provider(agent).analytics_capabilities()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsResponseUsage {
    pub index: u64,
    pub timestamp: String,
    pub model: String,
    pub usage: AnalyticsTokenUsage,
    pub cumulative: AnalyticsTokenUsage,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsRun {
    pub start: String,
    pub end: String,
    pub completed: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsToolCall {
    pub timestamp: String,
    pub name: String,
    pub server: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsSkillCall {
    pub timestamp: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsLimitSample {
    pub timestamp: String,
    pub window_minutes: u32,
    pub used_percent: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAnalytics {
    pub session_id: String,
    pub agent: AgentKind,
    pub session_path: PathBuf,
    pub capabilities: Option<AnalyticsCapabilities>,
    pub responses: Vec<AnalyticsResponseUsage>,
    pub runs: Vec<AnalyticsRun>,
    pub tools: Vec<AnalyticsToolCall>,
    pub skills: Vec<AnalyticsSkillCall>,
    pub aborts: Vec<String>,
    pub compactions: Vec<String>,
    pub limit_samples: Vec<AnalyticsLimitSample>,
    pub malformed_lines: usize,
}

impl Default for SessionAnalytics {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            agent: AgentKind::Unknown,
            session_path: PathBuf::new(),
            capabilities: None,
            responses: Vec::new(),
            runs: Vec::new(),
            tools: Vec::new(),
            skills: Vec::new(),
            aborts: Vec::new(),
            compactions: Vec::new(),
            limit_samples: Vec::new(),
            malformed_lines: 0,
        }
    }
}

impl SessionAnalytics {
    pub fn capabilities(&self) -> AnalyticsCapabilities {
        self.capabilities
            .unwrap_or_else(|| AnalyticsCapabilities::for_agent(self.agent))
    }

    pub(crate) fn snapshot_runs(&self, state: &AnalyticsParserState) -> Vec<AnalyticsRun> {
        let mut runs = self.runs.clone();
        if let Some(start) = state.open_run.as_ref() {
            runs.push(AnalyticsRun {
                start: start.clone(),
                end: state.last_timestamp.clone(),
                completed: state.approximate_runs,
            });
        }
        runs
    }

    pub(crate) fn overview_index(&self, state: &AnalyticsParserState) -> AnalyticsOverviewIndex {
        let mut first = None;
        let mut last = None;
        for timestamp in self
            .responses
            .iter()
            .map(|event| event.timestamp.as_str())
            .chain(self.runs.iter().map(|run| run.start.as_str()))
            .chain(self.tools.iter().map(|event| event.timestamp.as_str()))
            .chain(self.skills.iter().map(|event| event.timestamp.as_str()))
            .chain(self.aborts.iter().map(String::as_str))
            .chain(self.compactions.iter().map(String::as_str))
            .chain(
                self.limit_samples
                    .iter()
                    .map(|event| event.timestamp.as_str()),
            )
            .chain(state.open_run.iter().map(String::as_str))
        {
            update_coverage(timestamp, &mut first, &mut last);
        }
        AnalyticsOverviewIndex {
            first,
            last,
            has_activity: !self.responses.is_empty()
                || !self.runs.is_empty()
                || !self.tools.is_empty()
                || !self.aborts.is_empty()
                || !self.compactions.is_empty()
                || state.open_run.is_some(),
            capabilities: self.capabilities(),
        }
    }
}

pub(crate) fn overview_record(record: &SessionAnalyticsRecord) -> SessionAnalyticsOverviewRecord {
    let analytics = &record.analytics;
    let mut first = None;
    let mut last = None;
    let mut days = BTreeMap::<String, SessionAnalyticsOverviewDayAccumulator>::new();

    for response in &analytics.responses {
        update_coverage(&response.timestamp, &mut first, &mut last);
        let Some(date) = analytics_date(&response.timestamp).map(|date| date.to_string()) else {
            continue;
        };
        let day = days.entry(date).or_default();
        day.usage.add_assign(response.usage);
        day.responses += 1;
        day.has_response_or_run = true;
        let model = response.model.clone();
        let model_usage = day.models.entry(model).or_default();
        if model_usage.model.is_empty() {
            model_usage.model = response.model.clone();
        }
        model_usage.total_tokens = model_usage
            .total_tokens
            .saturating_add(response.usage.total_tokens);
        model_usage.responses += 1;
    }

    for run in analytics.snapshot_runs(&record.state) {
        update_coverage(&run.start, &mut first, &mut last);
        let Some(date) = analytics_date(&run.start).map(|date| date.to_string()) else {
            continue;
        };
        let day = days.entry(date).or_default();
        day.runs.push(run);
        day.has_response_or_run = true;
    }

    for timestamp in &analytics.aborts {
        update_coverage(timestamp, &mut first, &mut last);
        let Some(date) = analytics_date(timestamp).map(|date| date.to_string()) else {
            continue;
        };
        days.entry(date).or_default().aborted += 1;
    }

    for timestamp in &analytics.compactions {
        update_coverage(timestamp, &mut first, &mut last);
        let Some(date) = analytics_date(timestamp).map(|date| date.to_string()) else {
            continue;
        };
        days.entry(date).or_default().compacted += 1;
    }

    for sample in &analytics.limit_samples {
        update_coverage(&sample.timestamp, &mut first, &mut last);
        let Some(date) = analytics_date(&sample.timestamp).map(|date| date.to_string()) else {
            continue;
        };
        let day = days.entry(date).or_default();
        let previous = day
            .rate_limits
            .get(&sample.window_minutes)
            .copied()
            .unwrap_or(0.0);
        day.rate_limits
            .insert(sample.window_minutes, previous.max(sample.used_percent));
    }

    for call in &analytics.tools {
        update_coverage(&call.timestamp, &mut first, &mut last);
        let Some(date) = analytics_date(&call.timestamp).map(|date| date.to_string()) else {
            continue;
        };
        if call.name.is_empty() {
            continue;
        }
        let day = days.entry(date).or_default();
        let key = (call.server.clone(), call.name.clone());
        *day.tools.entry(key).or_default() += 1;
    }

    for call in &analytics.skills {
        update_coverage(&call.timestamp, &mut first, &mut last);
        let Some(date) = analytics_date(&call.timestamp).map(|date| date.to_string()) else {
            continue;
        };
        if call.name.is_empty() {
            continue;
        }
        let day = days.entry(date).or_default();
        *day.skills.entry(call.name.clone()).or_default() += 1;
    }

    SessionAnalyticsOverviewRecord {
        session_id: analytics.session_id.clone(),
        agent: analytics.agent,
        session_path: analytics.session_path.clone(),
        capabilities: analytics.capabilities(),
        first,
        last,
        has_activity: !analytics.responses.is_empty()
            || !analytics.runs.is_empty()
            || !analytics.tools.is_empty()
            || !analytics.aborts.is_empty()
            || !analytics.compactions.is_empty()
            || record.state.open_run.is_some(),
        days: days
            .into_iter()
            .map(|(date, day)| (date, day.finish()))
            .collect(),
    }
}

#[derive(Default)]
struct SessionAnalyticsOverviewDayAccumulator {
    usage: AnalyticsTokenUsage,
    responses: u64,
    runs: Vec<AnalyticsRun>,
    aborted: u64,
    compacted: u64,
    models: BTreeMap<String, SessionAnalyticsOverviewModel>,
    tools: BTreeMap<(String, String), u64>,
    skills: BTreeMap<String, u64>,
    rate_limits: BTreeMap<u32, f64>,
    has_response_or_run: bool,
}

impl SessionAnalyticsOverviewDayAccumulator {
    fn finish(self) -> SessionAnalyticsOverviewDay {
        SessionAnalyticsOverviewDay {
            usage: self.usage,
            responses: self.responses,
            runs: self.runs,
            aborted: self.aborted,
            compacted: self.compacted,
            models: self.models.into_values().collect(),
            tools: self
                .tools
                .into_iter()
                .map(|((server, name), calls)| AnalyticsCallUsage {
                    name,
                    server,
                    calls,
                })
                .collect(),
            skills: self
                .skills
                .into_iter()
                .map(|(name, calls)| AnalyticsCallUsage {
                    name,
                    server: String::new(),
                    calls,
                })
                .collect(),
            rate_limits: self.rate_limits,
            has_response_or_run: self.has_response_or_run,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnalyticsOverviewIndex {
    pub first: Option<String>,
    pub last: Option<String>,
    pub has_activity: bool,
    pub capabilities: AnalyticsCapabilities,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnalyticsParserState {
    #[serde(default)]
    parser_version: u32,
    pub(crate) previous_usage: AnalyticsTokenUsage,
    pub(crate) cumulative_usage: AnalyticsTokenUsage,
    pub(crate) current_model: String,
    pub(crate) open_run: Option<String>,
    pub(crate) last_timestamp: String,
    seen_usage_ids: BTreeSet<String>,
    seen_tool_ids: BTreeSet<String>,
    pub(crate) response_index: u64,
    approximate_runs: bool,
    #[serde(default)]
    source_device: u64,
    #[serde(default)]
    source_inode: u64,
    #[serde(default)]
    source_prefix_hash: String,
    #[serde(default)]
    source_boundary_hash: String,
    #[serde(default)]
    pub(crate) last_usage_key: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionAnalyticsRecord {
    pub analytics: SessionAnalytics,
    pub state: AnalyticsParserState,
    pub file_mtime: i64,
    pub file_size: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct SessionAnalyticsOverviewRecord {
    pub session_id: String,
    pub agent: AgentKind,
    pub session_path: PathBuf,
    pub capabilities: AnalyticsCapabilities,
    pub first: Option<String>,
    pub last: Option<String>,
    pub has_activity: bool,
    pub days: BTreeMap<String, SessionAnalyticsOverviewDay>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub(crate) struct SessionAnalyticsOverviewDay {
    pub usage: AnalyticsTokenUsage,
    pub responses: u64,
    pub runs: Vec<AnalyticsRun>,
    pub aborted: u64,
    pub compacted: u64,
    pub models: Vec<SessionAnalyticsOverviewModel>,
    pub tools: Vec<AnalyticsCallUsage>,
    pub skills: Vec<AnalyticsCallUsage>,
    pub rate_limits: BTreeMap<u32, f64>,
    pub has_response_or_run: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub(crate) struct SessionAnalyticsOverviewModel {
    pub model: String,
    pub total_tokens: u64,
    pub responses: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsRefreshReport {
    pub total: usize,
    pub parsed: usize,
    pub appended: usize,
    pub skipped: usize,
    pub failed: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsRefreshProgress {
    pub total: usize,
    pub completed: usize,
    pub parsed: usize,
    pub appended: usize,
    pub skipped: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsRunSummary {
    pub started: u64,
    pub completed: u64,
    pub unclosed: u64,
    pub total_ms: u64,
    pub max_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsModelUsage {
    pub model: String,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsCallUsage {
    pub name: String,
    pub server: String,
    pub calls: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsDay {
    pub date: String,
    pub usage: AnalyticsTokenUsage,
    pub responses: u64,
    pub sessions: usize,
    pub sessions_by_agent: BTreeMap<AgentKind, usize>,
    pub runs: AnalyticsRunSummary,
    pub aborted: u64,
    pub compacted: u64,
    pub models: Vec<AnalyticsModelUsage>,
    pub tools: Vec<AnalyticsCallUsage>,
    pub skills: Vec<AnalyticsCallUsage>,
    pub rate_limits: BTreeMap<u32, f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsRankItem {
    pub name: String,
    pub server: String,
    pub calls: u64,
    pub sessions: usize,
    pub share: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsProviderCapability {
    pub agent: AgentKind,
    #[serde(flatten)]
    pub capabilities: AnalyticsCapabilities,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsOverviewSummary {
    pub usage: AnalyticsTokenUsage,
    pub responses: u64,
    pub sessions: usize,
    pub runs: AnalyticsRunSummary,
    pub aborted: u64,
    pub aborted_rate: f64,
    pub compacted: u64,
    pub compacted_sessions: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsCoverage {
    pub first: Option<String>,
    pub last: Option<String>,
    pub total_sessions: usize,
    pub analyzed_sessions: usize,
    pub indexing_sessions: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverviewAnalytics {
    pub revision: u64,
    pub generated_at: String,
    pub days_requested: u32,
    pub rank_days: u32,
    pub coverage: AnalyticsCoverage,
    pub capabilities: Vec<AnalyticsProviderCapability>,
    pub summary: AnalyticsOverviewSummary,
    pub days: Vec<AnalyticsDay>,
    pub tools: Vec<AnalyticsRankItem>,
    pub skills: Vec<AnalyticsRankItem>,
    pub warnings: Vec<String>,
}

#[derive(Default)]
struct DayAccumulator {
    usage: AnalyticsTokenUsage,
    responses: u64,
    runs: AnalyticsRunSummary,
    aborted: u64,
    compacted: u64,
    models: BTreeMap<String, u64>,
    tools: BTreeMap<(String, String), u64>,
    skills: BTreeMap<String, u64>,
    rate_limits: BTreeMap<u32, f64>,
    sessions: BTreeSet<String>,
    sessions_by_agent: BTreeMap<AgentKind, BTreeSet<String>>,
}

#[derive(Default)]
struct RankAccumulator {
    name: String,
    server: String,
    calls: u64,
    sessions: BTreeSet<String>,
}

#[cfg(test)]
pub(crate) fn aggregate_overview(
    records: &[SessionAnalyticsRecord],
    days_requested: u32,
    rank_days: u32,
    warnings: Vec<String>,
) -> OverviewAnalytics {
    // The overview UI requests the exact span back to the earliest transcript.
    // Keep a defensive 100-year ceiling for malformed direct command calls.
    let days_requested = days_requested.clamp(1, 365);
    let rank_days = rank_days.clamp(1, 730);
    let today = Local::now().date_naive();
    let since = today - Duration::days(i64::from(days_requested.saturating_sub(1)));
    let rank_since = today - Duration::days(i64::from(rank_days.saturating_sub(1)));
    let mut by_day = BTreeMap::<String, DayAccumulator>::new();
    let mut tools = BTreeMap::<String, RankAccumulator>::new();
    let mut skills = BTreeMap::<String, RankAccumulator>::new();
    let mut capabilities = BTreeMap::<AgentKind, AnalyticsCapabilities>::new();
    let mut first = None::<String>;
    let mut last = None::<String>;
    let mut analyzed_sessions = 0;
    let mut compacted_sessions = BTreeSet::new();

    for record in records {
        let analytics = &record.analytics;
        let identity = format!(
            "{}\0{}\0{}",
            agent_key(analytics.agent),
            analytics.session_id,
            analytics.session_path.display()
        );
        capabilities
            .entry(analytics.agent)
            .or_insert_with(|| analytics.capabilities());
        if !analytics.responses.is_empty()
            || !analytics.runs.is_empty()
            || !analytics.tools.is_empty()
            || !analytics.aborts.is_empty()
            || !analytics.compactions.is_empty()
            || record.state.open_run.is_some()
        {
            analyzed_sessions += 1;
        }

        for response in &analytics.responses {
            update_coverage(&response.timestamp, &mut first, &mut last);
            let Some(date) = analytics_date(&response.timestamp) else {
                continue;
            };
            if date < since {
                continue;
            }
            let slot = by_day.entry(date.to_string()).or_default();
            slot.usage.add_assign(response.usage);
            slot.responses += 1;
            record_session(slot, analytics.agent, &identity);
            *slot
                .models
                .entry(response.model.clone())
                .or_default() += response.usage.total_tokens;
        }
        for run in analytics.snapshot_runs(&record.state) {
            update_coverage(&run.start, &mut first, &mut last);
            let Some(date) = analytics_date(&run.start) else {
                continue;
            };
            if date < since {
                continue;
            }
            let slot = by_day.entry(date.to_string()).or_default();
            record_session(slot, analytics.agent, &identity);
            add_run(&mut slot.runs, &run);
        }
        for timestamp in &analytics.aborts {
            update_coverage(timestamp, &mut first, &mut last);
            if let Some(date) = analytics_date(timestamp).filter(|date| *date >= since) {
                let slot = by_day.entry(date.to_string()).or_default();
                slot.aborted += 1;
                record_session(slot, analytics.agent, &identity);
            }
        }
        for timestamp in &analytics.compactions {
            update_coverage(timestamp, &mut first, &mut last);
            if let Some(date) = analytics_date(timestamp).filter(|date| *date >= since) {
                let slot = by_day.entry(date.to_string()).or_default();
                slot.compacted += 1;
                record_session(slot, analytics.agent, &identity);
                compacted_sessions.insert(identity.clone());
            }
        }
        for sample in &analytics.limit_samples {
            update_coverage(&sample.timestamp, &mut first, &mut last);
            let Some(date) = analytics_date(&sample.timestamp) else {
                continue;
            };
            if date < since {
                continue;
            }
            let slot = by_day.entry(date.to_string()).or_default();
            let previous = slot
                .rate_limits
                .get(&sample.window_minutes)
                .copied()
                .unwrap_or(0.0);
            slot.rate_limits
                .insert(sample.window_minutes, previous.max(sample.used_percent));
        }
        for call in &analytics.tools {
            update_coverage(&call.timestamp, &mut first, &mut last);
            let Some(date) = analytics_date(&call.timestamp) else {
                continue;
            };
            if call.name.is_empty() {
                continue;
            }
            if date >= since {
                let slot = by_day.entry(date.to_string()).or_default();
                *slot
                    .tools
                    .entry((call.server.clone(), call.name.clone()))
                    .or_default() += 1;
                record_session(slot, analytics.agent, &identity);
            }
            if date < rank_since {
                continue;
            }
            let key = format!("{}\0{}", call.server, call.name);
            let slot = tools.entry(key).or_default();
            slot.name = call.name.clone();
            slot.calls += 1;
            slot.sessions.insert(identity.clone());
            if slot.server.is_empty() && !call.server.is_empty() {
                slot.server = call.server.clone();
            }
        }
        for call in &analytics.skills {
            update_coverage(&call.timestamp, &mut first, &mut last);
            let Some(date) = analytics_date(&call.timestamp) else {
                continue;
            };
            if call.name.is_empty() {
                continue;
            }
            if date >= since {
                let slot = by_day.entry(date.to_string()).or_default();
                *slot.skills.entry(call.name.clone()).or_default() += 1;
                record_session(slot, analytics.agent, &identity);
            }
            if date < rank_since {
                continue;
            }
            let slot = skills.entry(call.name.clone()).or_default();
            slot.name = call.name.clone();
            slot.calls += 1;
            slot.sessions.insert(identity.clone());
        }
    }

    let mut days = Vec::with_capacity(days_requested as usize);
    let mut cursor = since;
    while cursor <= today {
        let key = cursor.to_string();
        let slot = by_day.remove(&key).unwrap_or_default();
        days.push(AnalyticsDay {
            date: key,
            usage: slot.usage,
            responses: slot.responses,
            sessions: slot.sessions.len(),
            sessions_by_agent: slot
                .sessions_by_agent
                .into_iter()
                .map(|(agent, sessions)| (agent, sessions.len()))
                .collect(),
            runs: slot.runs,
            aborted: slot.aborted,
            compacted: slot.compacted,
            models: slot
                .models
                .into_iter()
                .map(|(model, total_tokens)| AnalyticsModelUsage {
                    model,
                    total_tokens,
                })
                .collect(),
            tools: slot
                .tools
                .into_iter()
                .map(|((server, name), calls)| AnalyticsCallUsage {
                    name,
                    server,
                    calls,
                })
                .collect(),
            skills: slot
                .skills
                .into_iter()
                .map(|(name, calls)| AnalyticsCallUsage {
                    name,
                    server: String::new(),
                    calls,
                })
                .collect(),
            rate_limits: slot.rate_limits,
        });
        cursor += Duration::days(1);
    }

    let mut summary = AnalyticsOverviewSummary {
        sessions: days.iter().map(|day| day.sessions).max().unwrap_or(0),
        compacted_sessions: compacted_sessions.len(),
        ..AnalyticsOverviewSummary::default()
    };
    let mut summary_sessions = BTreeSet::new();
    for day in &days {
        summary.usage.add_assign(day.usage);
        summary.responses += day.responses;
        add_run_summary(&mut summary.runs, &day.runs);
        summary.aborted += day.aborted;
        summary.compacted += day.compacted;
    }
    for record in records {
        let has_in_window = record
            .analytics
            .responses
            .iter()
            .any(|event| analytics_date(&event.timestamp).is_some_and(|date| date >= since))
            || record
                .analytics
                .snapshot_runs(&record.state)
                .iter()
                .any(|run| analytics_date(&run.start).is_some_and(|date| date >= since));
        if has_in_window {
            summary_sessions.insert(format!(
                "{}\0{}\0{}",
                agent_key(record.analytics.agent),
                record.analytics.session_id,
                record.analytics.session_path.display()
            ));
        }
    }
    summary.sessions = summary_sessions.len();
    summary.aborted_rate = if summary.runs.started > 0 {
        summary.aborted as f64 / summary.runs.started as f64
    } else {
        0.0
    };

    OverviewAnalytics {
        revision: 0,
        generated_at: Local::now().to_rfc3339(),
        days_requested,
        rank_days,
        coverage: AnalyticsCoverage {
            first,
            last,
            total_sessions: records.len(),
            analyzed_sessions,
            indexing_sessions: 0,
        },
        capabilities: capabilities
            .into_iter()
            .map(|(agent, capabilities)| AnalyticsProviderCapability {
                agent,
                capabilities,
            })
            .collect(),
        summary,
        days,
        tools: finalize_rank(tools),
        skills: finalize_rank(skills),
        warnings,
    }
}

pub(crate) fn aggregate_overview_records(
    records: &[SessionAnalyticsOverviewRecord],
    days_requested: u32,
    rank_days: u32,
    warnings: Vec<String>,
) -> OverviewAnalytics {
    let days_requested = days_requested.clamp(1, 365);
    let rank_days = rank_days.clamp(1, 730);
    let today = Local::now().date_naive();
    let since = today - Duration::days(i64::from(days_requested.saturating_sub(1)));
    let rank_since = today - Duration::days(i64::from(rank_days.saturating_sub(1)));
    let mut by_day = BTreeMap::<String, DayAccumulator>::new();
    let mut tools = BTreeMap::<String, RankAccumulator>::new();
    let mut skills = BTreeMap::<String, RankAccumulator>::new();
    let mut capabilities = BTreeMap::<AgentKind, AnalyticsCapabilities>::new();
    let mut compacted_sessions = BTreeSet::new();
    let mut first = None;
    let mut last = None;
    let mut analyzed_sessions = 0;

    for record in records {
        if let Some(value) = record.first.as_ref()
            && first.as_ref().is_none_or(|current| value < current)
        {
            first = Some(value.clone());
        }
        if let Some(value) = record.last.as_ref()
            && last.as_ref().is_none_or(|current| value > current)
        {
            last = Some(value.clone());
        }
        capabilities
            .entry(record.agent)
            .or_insert(record.capabilities);
        analyzed_sessions += usize::from(record.has_activity);
        let identity = format!(
            "{}\0{}\0{}",
            agent_key(record.agent),
            record.session_id,
            record.session_path.display()
        );

        for (date, contribution) in &record.days {
            let Ok(date_value) = date.parse::<NaiveDate>() else {
                continue;
            };
            if date_value < since {
                continue;
            }
            let slot = by_day.entry(date.clone()).or_default();
            slot.usage.add_assign(contribution.usage);
            slot.responses += contribution.responses;
            if contribution.responses > 0
                || !contribution.runs.is_empty()
                || contribution.aborted > 0
                || contribution.compacted > 0
                || !contribution.tools.is_empty()
                || !contribution.skills.is_empty()
            {
                record_session(slot, record.agent, &identity);
            }
            for model in &contribution.models {
                *slot.models.entry(model.model.clone()).or_default() += model.total_tokens;
            }
            for run in &contribution.runs {
                add_run(&mut slot.runs, run);
            }
            slot.aborted += contribution.aborted;
            slot.compacted += contribution.compacted;
            if contribution.compacted > 0 {
                compacted_sessions.insert(identity.clone());
            }
            for (window, used_percent) in &contribution.rate_limits {
                let previous = slot.rate_limits.get(window).copied().unwrap_or(0.0);
                slot.rate_limits
                    .insert(*window, previous.max(*used_percent));
            }
            for call in &contribution.tools {
                *slot
                    .tools
                    .entry((call.server.clone(), call.name.clone()))
                    .or_default() += call.calls;
            }
            for call in &contribution.skills {
                *slot.skills.entry(call.name.clone()).or_default() += call.calls;
            }

            if date_value >= rank_since {
                for call in &contribution.tools {
                    let key = format!("{}\0{}", call.server, call.name);
                    let rank = tools.entry(key).or_default();
                    rank.name = call.name.clone();
                    rank.server = call.server.clone();
                    rank.calls += call.calls;
                    rank.sessions.insert(identity.clone());
                }
                for call in &contribution.skills {
                    let rank = skills.entry(call.name.clone()).or_default();
                    rank.name = call.name.clone();
                    rank.calls += call.calls;
                    rank.sessions.insert(identity.clone());
                }
            }
        }
    }

    let mut days = Vec::with_capacity(days_requested as usize);
    let mut cursor = since;
    while cursor <= today {
        let key = cursor.to_string();
        let slot = by_day.remove(&key).unwrap_or_default();
        days.push(AnalyticsDay {
            date: key,
            usage: slot.usage,
            responses: slot.responses,
            sessions: slot.sessions.len(),
            sessions_by_agent: slot
                .sessions_by_agent
                .into_iter()
                .map(|(agent, sessions)| (agent, sessions.len()))
                .collect(),
            runs: slot.runs,
            aborted: slot.aborted,
            compacted: slot.compacted,
            models: slot
                .models
                .into_iter()
                .map(|(model, total_tokens)| AnalyticsModelUsage {
                    model,
                    total_tokens,
                })
                .collect(),
            tools: slot
                .tools
                .into_iter()
                .map(|((server, name), calls)| AnalyticsCallUsage {
                    name,
                    server,
                    calls,
                })
                .collect(),
            skills: slot
                .skills
                .into_iter()
                .map(|(name, calls)| AnalyticsCallUsage {
                    name,
                    server: String::new(),
                    calls,
                })
                .collect(),
            rate_limits: slot.rate_limits,
        });
        cursor += Duration::days(1);
    }

    let mut summary = AnalyticsOverviewSummary {
        compacted_sessions: compacted_sessions.len(),
        ..AnalyticsOverviewSummary::default()
    };
    let mut summary_sessions = BTreeSet::new();
    for day in &days {
        summary.usage.add_assign(day.usage);
        summary.responses += day.responses;
        add_run_summary(&mut summary.runs, &day.runs);
        summary.aborted += day.aborted;
        summary.compacted += day.compacted;
    }
    for record in records {
        if record.days.iter().any(|(date, contribution)| {
            date.parse::<NaiveDate>()
                .is_ok_and(|date| date >= since && contribution.has_response_or_run)
        }) {
            summary_sessions.insert(format!(
                "{}\0{}\0{}",
                agent_key(record.agent),
                record.session_id,
                record.session_path.display()
            ));
        }
    }
    summary.sessions = summary_sessions.len();
    summary.aborted_rate = if summary.runs.started > 0 {
        summary.aborted as f64 / summary.runs.started as f64
    } else {
        0.0
    };

    OverviewAnalytics {
        revision: 0,
        generated_at: Local::now().to_rfc3339(),
        days_requested,
        rank_days,
        coverage: AnalyticsCoverage {
            first,
            last,
            total_sessions: records.len(),
            analyzed_sessions,
            indexing_sessions: 0,
        },
        capabilities: capabilities
            .into_iter()
            .map(|(agent, capabilities)| AnalyticsProviderCapability {
                agent,
                capabilities,
            })
            .collect(),
        summary,
        days,
        tools: finalize_rank(tools),
        skills: finalize_rank(skills),
        warnings,
    }
}

fn record_session(slot: &mut DayAccumulator, agent: AgentKind, identity: &str) {
    slot.sessions.insert(identity.to_string());
    slot.sessions_by_agent
        .entry(agent)
        .or_default()
        .insert(identity.to_string());
}

fn add_run(summary: &mut AnalyticsRunSummary, run: &AnalyticsRun) {
    summary.started += 1;
    if !run.completed {
        summary.unclosed += 1;
        return;
    }
    summary.completed += 1;
    let elapsed = analytics_timestamp(&run.start)
        .zip(analytics_timestamp(&run.end))
        .map(|(start, end)| end.signed_duration_since(start).num_milliseconds().max(0) as u64)
        .unwrap_or(0);
    summary.total_ms = summary.total_ms.saturating_add(elapsed);
    summary.max_ms = summary.max_ms.max(elapsed);
}

fn add_run_summary(target: &mut AnalyticsRunSummary, source: &AnalyticsRunSummary) {
    target.started += source.started;
    target.completed += source.completed;
    target.unclosed += source.unclosed;
    target.total_ms = target.total_ms.saturating_add(source.total_ms);
    target.max_ms = target.max_ms.max(source.max_ms);
}

fn finalize_rank(values: BTreeMap<String, RankAccumulator>) -> Vec<AnalyticsRankItem> {
    let total = values.values().map(|value| value.calls).sum::<u64>();
    let mut values = values
        .into_iter()
        .map(|(key, value)| AnalyticsRankItem {
            name: if value.name.is_empty() {
                key
            } else {
                value.name
            },
            server: value.server,
            calls: value.calls,
            sessions: value.sessions.len(),
            share: if total > 0 {
                value.calls as f64 / total as f64
            } else {
                0.0
            },
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        right
            .calls
            .cmp(&left.calls)
            .then_with(|| left.name.cmp(&right.name))
    });
    values.truncate(20);
    values
}

fn update_coverage(timestamp: &str, first: &mut Option<String>, last: &mut Option<String>) {
    let Some(date) = analytics_date(timestamp).map(|date| date.to_string()) else {
        return;
    };
    if first.as_ref().is_none_or(|value| date < *value) {
        *first = Some(date.clone());
    }
    if last.as_ref().is_none_or(|value| date > *value) {
        *last = Some(date);
    }
}

fn analytics_timestamp(timestamp: &str) -> Option<DateTime<chrono::FixedOffset>> {
    parse_timestamp(timestamp)
}

fn analytics_date(timestamp: &str) -> Option<NaiveDate> {
    analytics_timestamp(timestamp).map(|timestamp| timestamp.with_timezone(&Local).date_naive())
}

fn agent_key(agent: AgentKind) -> &'static str {
    crate::providers::agent_provider(agent).storage_key()
}

pub(crate) fn diff_usage(
    previous: AnalyticsTokenUsage,
    current: AnalyticsTokenUsage,
) -> AnalyticsTokenUsage {
    if current.total_tokens < previous.total_tokens {
        return current;
    }
    AnalyticsTokenUsage {
        input_tokens: current.input_tokens.saturating_sub(previous.input_tokens),
        cached_input_tokens: current
            .cached_input_tokens
            .saturating_sub(previous.cached_input_tokens),
        cache_write_input_tokens: current
            .cache_write_input_tokens
            .saturating_sub(previous.cache_write_input_tokens),
        output_tokens: current.output_tokens.saturating_sub(previous.output_tokens),
        reasoning_output_tokens: current
            .reasoning_output_tokens
            .saturating_sub(previous.reasoning_output_tokens),
        total_tokens: current.total_tokens.saturating_sub(previous.total_tokens),
    }
}

pub(crate) fn analyze_session(
    session: &SessionRecord,
    previous: Option<&SessionAnalyticsRecord>,
) -> Result<SessionAnalyticsRecord> {
    let mut file = File::open(&session.path)
        .with_context(|| format!("failed to open {}", session.path.display()))?;
    let (file_mtime, file_size) = analytics_file_state(&file, &session.path)?;
    let (source_device, source_inode) = analytics_file_identity(&file)?;
    let is_jsonl = session
        .path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"));
    if !is_jsonl {
        return Ok(empty_record(session, file_mtime, file_size));
    }

    let append_fingerprint = previous
        .filter(|record| record.file_size >= 0)
        .map(|record| analytics_source_hashes(&mut file, record.file_size as u64))
        .transpose()?;
    let can_append = previous
        .zip(append_fingerprint.as_ref())
        .is_some_and(|(record, hashes)| {
            !record.state.source_prefix_hash.is_empty()
                && !record.state.source_boundary_hash.is_empty()
                && record.state.source_device == source_device
                && record.state.source_inode == source_inode
                && record.state.source_prefix_hash == hashes.0
                && record.state.source_boundary_hash == hashes.1
                && record.state.parser_version == ANALYTICS_PARSER_VERSION
                && record.file_size >= 0
                && record.file_size < file_size
                && record.file_mtime <= file_mtime
                && record.analytics.session_id == session.id
                && record.analytics.agent == session.agent
                && record.analytics.session_path == session.path
                && is_line_boundary(&session.path, record.file_size as u64)
        });
    let mut record = if can_append {
        previous.cloned().expect("append record checked")
    } else {
        empty_record(session, file_mtime, file_size)
    };
    record.analytics.capabilities = Some(AnalyticsCapabilities::for_agent(session.agent));
    record.state.approximate_runs = !record.analytics.capabilities().explicit_runs;

    let offset = if can_append {
        record.file_size as u64
    } else {
        0
    };
    file.seek(SeekFrom::Start(offset))
        .with_context(|| format!("failed to seek {}", session.path.display()))?;
    let remaining = (file_size as u64).saturating_sub(offset);
    let mut reader = BufReader::new(file.take(remaining));
    let mut line = String::new();
    let mut indexed_size = file_size;
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            break;
        }
        let has_trailing_newline = line.ends_with('\n');
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() {
            continue;
        }
        if !has_trailing_newline && serde_json::from_str::<Value>(line).is_err() {
            indexed_size = file_size.saturating_sub(bytes as i64);
            break;
        }
        crate::providers::agent_provider(session.agent).parse_analytics_line(&line, &mut record);
    }

    let mut file = reader.into_inner().into_inner();
    let (source_prefix_hash, source_boundary_hash) =
        analytics_source_hashes(&mut file, indexed_size.max(0) as u64)?;

    if record.state.current_model.is_empty()
        && let Some(model) = session
            .model
            .as_deref()
            .filter(|model| !model.trim().is_empty())
    {
        set_model(&mut record, model);
    }
    record.file_mtime = file_mtime;
    record.file_size = indexed_size;
    record.state.source_device = source_device;
    record.state.source_inode = source_inode;
    record.state.source_prefix_hash = source_prefix_hash;
    record.state.source_boundary_hash = source_boundary_hash;
    Ok(record)
}

fn empty_record(
    session: &SessionRecord,
    file_mtime: i64,
    file_size: i64,
) -> SessionAnalyticsRecord {
    let capabilities = AnalyticsCapabilities::for_agent(session.agent);
    SessionAnalyticsRecord {
        analytics: SessionAnalytics {
            session_id: session.id.clone(),
            agent: session.agent,
            session_path: session.path.clone(),
            capabilities: Some(capabilities),
            ..SessionAnalytics::default()
        },
        state: AnalyticsParserState {
            parser_version: ANALYTICS_PARSER_VERSION,
            current_model: session.model.clone().unwrap_or_default(),
            last_timestamp: session
                .started_at
                .clone()
                .or_else(|| session.updated_at.clone())
                .unwrap_or_default(),
            approximate_runs: !capabilities.explicit_runs,
            ..AnalyticsParserState::default()
        },
        file_mtime,
        file_size,
    }
}

fn analytics_file_state(file: &File, path: &Path) -> Result<(i64, i64)> {
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect analytics source {}", path.display()))?;
    let file_mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0);
    let file_size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
    Ok((file_mtime, file_size))
}

fn analytics_file_identity(file: &File) -> Result<(u64, u64)> {
    let metadata = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok((0, 0))
    }
}

fn analytics_source_hashes(file: &mut File, boundary: u64) -> Result<(String, String)> {
    const HASH_BYTES: u64 = 4 * 1024;
    fn hash_range(file: &mut File, start: u64, length: u64) -> Result<String> {
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::with_capacity(length as usize);
        file.take(length).read_to_end(&mut bytes)?;
        let mut digest = Sha256::new();
        digest.update(&bytes);
        Ok(format!("{:x}", digest.finalize()))
    }

    let prefix = hash_range(file, 0, HASH_BYTES)?;
    let boundary_start = boundary.saturating_sub(HASH_BYTES);
    let boundary_hash = hash_range(file, boundary_start, boundary - boundary_start)?;
    Ok((prefix, boundary_hash))
}

fn is_line_boundary(path: &Path, offset: u64) -> bool {
    if offset == 0 {
        return true;
    }
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    if file.seek(SeekFrom::Start(offset - 1)).is_err() {
        return false;
    }
    let mut byte = [0_u8; 1];
    std::io::Read::read_exact(&mut file, &mut byte).is_ok() && byte[0] == b'\n'
}

pub(crate) fn parse_message_line(line: &str, record: &mut SessionAnalyticsRecord) {
    parse_message_line_with_timestamp(line, record, None);
}

pub(crate) fn parse_message_line_with_timestamp(
    line: &str,
    record: &mut SessionAnalyticsRecord,
    timestamp_override: Option<String>,
) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        record.analytics.malformed_lines += 1;
        return;
    };
    let timestamp = timestamp_override
        .or_else(|| string_at(&value, &["timestamp"]))
        .filter(|timestamp| !timestamp.is_empty())
        .unwrap_or_else(|| record.state.last_timestamp.clone());
    let previous_stamp = record.state.last_timestamp.clone();
    if !timestamp.is_empty() {
        record.state.last_timestamp = timestamp.clone();
    }
    let entry_type = value
        .get("type")
        .or_else(|| value.get("role"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if entry_type == "user" {
        if value
            .get("isCompactSummary")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            if !timestamp.is_empty() {
                record.analytics.compactions.push(timestamp);
            }
            return;
        }
        let content = value.pointer("/message/content").unwrap_or(&Value::Null);
        let content_text = message_content_text(content);
        if content_text.contains("Request interrupted") {
            if !timestamp.is_empty() {
                record.analytics.aborts.push(timestamp.clone());
            }
            close_open_run(record, &timestamp, false);
            return;
        }
        if is_real_user_prompt(&value, content, &content_text) {
            let end = if previous_stamp.is_empty() {
                timestamp.as_str()
            } else {
                previous_stamp.as_str()
            };
            close_open_run(record, end, true);
            if !timestamp.is_empty() {
                record.state.open_run = Some(timestamp);
            }
        }
        return;
    }
    if entry_type != "assistant" {
        return;
    }

    let message = value.get("message").unwrap_or(&Value::Null);
    if let Some(model) = message.get("model").and_then(Value::as_str) {
        set_model(record, model);
    }
    if let Some(content) = message.get("content").and_then(Value::as_array) {
        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            record_message_tool(block, &timestamp, record);
        }
    }

    let Some(raw_usage) = message.get("usage") else {
        return;
    };
    let key = message
        .get("id")
        .or_else(|| value.get("requestId"))
        .or_else(|| value.get("uuid"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(key) = key {
        if !record.state.seen_usage_ids.insert(key) {
            return;
        }
    }
    let usage = message_usage(raw_usage);
    if usage.total_tokens == 0 {
        return;
    }
    record.state.cumulative_usage.add_assign(usage);
    record.state.response_index += 1;
    record.analytics.responses.push(AnalyticsResponseUsage {
        index: record.state.response_index,
        timestamp,
        model: record.state.current_model.clone(),
        usage,
        cumulative: record.state.cumulative_usage,
    });
}

pub(crate) fn set_model(record: &mut SessionAnalyticsRecord, model: &str) {
    let model = model.trim();
    if model.is_empty() {
        return;
    }
    record.state.current_model = model.to_string();
    for response in record
        .analytics
        .responses
        .iter_mut()
        .filter(|response| response.model.is_empty())
    {
        response.model = model.to_string();
    }
}

pub(crate) fn close_open_run(record: &mut SessionAnalyticsRecord, end: &str, completed: bool) {
    let Some(start) = record.state.open_run.take() else {
        return;
    };
    record.analytics.runs.push(AnalyticsRun {
        start,
        end: end.to_string(),
        completed,
    });
}

pub(crate) fn record_tool_call(
    payload: &Value,
    timestamp: &str,
    record: &mut SessionAnalyticsRecord,
) {
    let call_id = payload
        .get("call_id")
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str);
    if let Some(call_id) = call_id
        && !record.state.seen_tool_ids.insert(call_id.to_string())
    {
        return;
    }
    let raw_name = payload
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| payload.pointer("/action/type").and_then(Value::as_str))
        .unwrap_or_default();
    let (name, server) = normalize_tool_name(raw_name);
    record.analytics.tools.push(AnalyticsToolCall {
        timestamp: timestamp.to_string(),
        name: name.clone(),
        server,
    });
    record_explicit_skill(payload, &name, timestamp, record);
    let mut strings = Vec::new();
    collect_strings(payload, &mut strings);
    for text in strings {
        for name in extract_skill_names(text) {
            push_skill_call(record, timestamp, &name);
        }
    }
}

fn record_message_tool(block: &Value, timestamp: &str, record: &mut SessionAnalyticsRecord) {
    if let Some(id) = block.get("id").and_then(Value::as_str)
        && !record.state.seen_tool_ids.insert(id.to_string())
    {
        return;
    }
    let raw_name = block
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (name, server) = normalize_tool_name(raw_name);
    record.analytics.tools.push(AnalyticsToolCall {
        timestamp: timestamp.to_string(),
        name: name.clone(),
        server,
    });
    record_explicit_skill(block, &name, timestamp, record);
    let mut strings = Vec::new();
    collect_strings(block.get("input").unwrap_or(&Value::Null), &mut strings);
    for text in strings {
        for name in extract_skill_names(text) {
            push_skill_call(record, timestamp, &name);
        }
    }
}

fn record_explicit_skill(
    payload: &Value,
    tool_name: &str,
    timestamp: &str,
    record: &mut SessionAnalyticsRecord,
) {
    if !tool_name.eq_ignore_ascii_case("skill") {
        return;
    }
    let direct = payload
        .pointer("/input/skill")
        .or_else(|| payload.get("skill"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let argument = payload
        .get("arguments")
        .and_then(Value::as_str)
        .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
        .and_then(|arguments| {
            arguments
                .get("skill")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    if let Some(name) = direct.or(argument) {
        push_skill_call(record, timestamp, &name);
    }
}

fn push_skill_call(record: &mut SessionAnalyticsRecord, timestamp: &str, name: &str) {
    if record
        .analytics
        .skills
        .last()
        .is_some_and(|call| call.timestamp == timestamp && call.name == name)
    {
        return;
    }
    record.analytics.skills.push(AnalyticsSkillCall {
        timestamp: timestamp.to_string(),
        name: name.to_string(),
    });
}

pub(crate) fn record_rate_limits(
    payload: &Value,
    timestamp: &str,
    record: &mut SessionAnalyticsRecord,
) {
    let Some(rate_limits) = payload.get("rate_limits") else {
        return;
    };
    for key in ["primary", "secondary"] {
        let Some(slot) = rate_limits.get(key) else {
            continue;
        };
        let minutes = slot
            .get("window_minutes")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0);
        if minutes == 0 {
            continue;
        }
        let used_percent = slot
            .get("used_percent")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        record.analytics.limit_samples.push(AnalyticsLimitSample {
            timestamp: timestamp.to_string(),
            window_minutes: minutes,
            used_percent,
        });
    }
}

pub(crate) fn parser_state_is_current(serialized: &str) -> bool {
    serde_json::from_str::<AnalyticsParserState>(serialized)
        .is_ok_and(|state| state.parser_version == ANALYTICS_PARSER_VERSION)
}

fn message_usage(value: &Value) -> AnalyticsTokenUsage {
    let fresh = u64_field(value, "input_tokens");
    let cached = u64_field(value, "cache_read_input_tokens");
    let cache_write = u64_field(value, "cache_creation_input_tokens");
    let output = u64_field(value, "output_tokens");
    AnalyticsTokenUsage {
        input_tokens: fresh.saturating_add(cached),
        cached_input_tokens: cached,
        cache_write_input_tokens: cache_write,
        output_tokens: output,
        reasoning_output_tokens: 0,
        total_tokens: fresh
            .saturating_add(cached)
            .saturating_add(cache_write)
            .saturating_add(output),
    }
}

fn is_real_user_prompt(value: &Value, content: &Value, text: &str) -> bool {
    if value
        .get("isSidechain")
        .or_else(|| value.get("isCompactSummary"))
        .or_else(|| value.get("isMeta"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || text.trim().is_empty()
        || text.contains("Request interrupted")
    {
        return false;
    }
    !content.as_array().is_some_and(|blocks| {
        blocks
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
    })
}

fn message_content_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_tool_name(raw: &str) -> (String, String) {
    if let Some(rest) = raw.strip_prefix("mcp__")
        && let Some((server, tool)) = rest.split_once("__")
        && !server.is_empty()
        && !tool.is_empty()
    {
        return (tool.to_string(), server.to_string());
    }
    (raw.trim().to_string(), String::new())
}

pub(crate) fn extract_skill_names(text: &str) -> Vec<String> {
    let normalized = text.replace('\\', "/");
    let bytes = normalized.as_bytes();
    let needle = b"skills/";
    let mut names = BTreeSet::new();
    let mut offset = 0;
    while offset + needle.len() <= bytes.len() {
        let Some(found) = bytes[offset..]
            .windows(needle.len())
            .position(|window| window == needle)
        else {
            break;
        };
        let start = offset + found;
        let boundary_ok = start == 0
            || matches!(
                bytes[start - 1],
                b'/' | b'"' | b'\'' | b' ' | b'\t' | b'\r' | b'\n'
            );
        let name_start = start + needle.len();
        let name_end = bytes[name_start..]
            .iter()
            .position(|byte| matches!(byte, b'/' | b'"' | b'\'' | b' ' | b'\t' | b'\r' | b'\n'))
            .map(|length| name_start + length)
            .unwrap_or(bytes.len());
        if boundary_ok
            && name_end > name_start
            && let Some(raw) = normalized.get(name_start..name_end)
            && !raw.starts_with('.')
        {
            let name = strip_semver_suffix(raw);
            if !name.is_empty() {
                names.insert(name.to_string());
            }
        }
        offset = name_start.max(start + 1);
    }
    names.into_iter().collect()
}

fn strip_semver_suffix(value: &str) -> &str {
    let Some(index) = value.rfind('-') else {
        return value;
    };
    let suffix = &value[index + 1..];
    if suffix.contains('.')
        && suffix
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
    {
        &value[..index]
    } else {
        value
    }
}

fn collect_strings<'a>(value: &'a Value, output: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => output.push(text),
        Value::Array(values) => {
            for value in values {
                collect_strings(value, output);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_strings(value, output);
            }
        }
        _ => {}
    }
}

pub(crate) fn bytes_contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

pub(crate) fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    current.as_str().map(str::to_string)
}

fn u64_field(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;
    use chrono::{Offset, TimeZone};

    fn temp_dir(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{name}-{suffix}"))
    }

    fn session(path: &Path, agent: AgentKind) -> SessionRecord {
        SessionRecord {
            id: "session-1".to_string(),
            agent,
            title: None,
            project: None,
            repository: None,
            repository_url: None,
            logical_project_id: None,
            logical_project_name: None,
            path: path.to_path_buf(),
            started_at: None,
            updated_at: None,
            message_count: None,
            first_user_message: None,
            last_user_message: None,
            last_assistant_message: None,
            turn_count: None,
            model: None,
            mode: None,
            approval_mode: None,
            is_run_everything: None,
            parent_session_id: None,
            token_usage: None,
        }
    }

    #[test]
    fn cumulative_usage_diff_ignores_duplicates_and_handles_reset() {
        let first = AnalyticsTokenUsage {
            input_tokens: 100,
            total_tokens: 120,
            ..AnalyticsTokenUsage::default()
        };
        assert_eq!(diff_usage(first, first).total_tokens, 0);
        let reset = AnalyticsTokenUsage {
            input_tokens: 10,
            total_tokens: 12,
            ..AnalyticsTokenUsage::default()
        };
        assert_eq!(diff_usage(first, reset), reset);
    }

    #[test]
    fn codex_parser_uses_last_usage_for_non_monotonic_cumulative_totals() {
        let root = temp_dir("tendi-analytics-codex-last-usage");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-session-1.jsonl");
        let events = [
            ("2026-08-01T01:00:00Z", 1_000_u64, 100_u64),
            ("2026-08-01T01:00:01Z", 2_000, 120),
            ("2026-08-01T01:00:02Z", 2_000, 120),
            ("2026-08-01T01:00:03Z", 1_500, 130),
            ("2026-08-01T01:00:04Z", 2_500, 140),
        ];
        let content = events
            .iter()
            .map(|(timestamp, cumulative, last)| {
                serde_json::json!({
                    "timestamp": timestamp,
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": { "total_tokens": cumulative },
                            "last_token_usage": { "total_tokens": last },
                        },
                    },
                })
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, format!("{content}\n")).unwrap();

        let parsed = analyze_session(&session(&path, AgentKind::Codex), None).unwrap();

        assert_eq!(
            parsed
                .analytics
                .responses
                .iter()
                .map(|response| response.usage.total_tokens)
                .collect::<Vec<_>>(),
            vec![100, 120, 130, 140]
        );
        assert_eq!(
            parsed
                .analytics
                .responses
                .last()
                .map(|response| response.cumulative.total_tokens),
            Some(490)
        );
        assert_eq!(parsed.state.parser_version, ANALYTICS_PARSER_VERSION);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_parser_tracks_models_runs_health_and_rate_windows() {
        let root = temp_dir("tendi-analytics-codex");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-session-1.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-one\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":20,\"reasoning_output_tokens\":5,\"total_tokens\":120}},\"rate_limits\":{\"primary\":{\"window_minutes\":10080,\"used_percent\":42}}}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":20,\"reasoning_output_tokens\":5,\"total_tokens\":120}}}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:04Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-two\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:05Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":180,\"cached_input_tokens\":140,\"output_tokens\":40,\"reasoning_output_tokens\":9,\"total_tokens\":220}}}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:06Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"context_compacted\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:07Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n"
            ),
        )
        .unwrap();

        let parsed = analyze_session(&session(&path, AgentKind::Codex), None).unwrap();
        assert_eq!(parsed.analytics.responses.len(), 2);
        assert_eq!(parsed.analytics.responses[0].model, "gpt-one");
        assert_eq!(parsed.analytics.responses[1].model, "gpt-two");
        assert_eq!(parsed.analytics.responses[1].usage.total_tokens, 100);
        assert_eq!(parsed.analytics.runs.len(), 1);
        assert!(parsed.analytics.runs[0].completed);
        assert_eq!(parsed.analytics.compactions.len(), 1);
        assert_eq!(parsed.analytics.limit_samples[0].window_minutes, 10080);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claude_usage_is_deduplicated_without_deduplicating_tools() {
        let root = temp_dir("tendi-analytics-claude");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session-1.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"user\",\"message\":{\"content\":\"do it\"}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:01Z\",\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"model\":\"claude-one\",\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":20,\"cache_creation_input_tokens\":5,\"output_tokens\":2},\"content\":[{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Read\",\"input\":{\"path\":\"/tmp/skills/foo-1.2.3/SKILL.md\"}}]}}\n",
                "{\"timestamp\":\"2026-08-01T01:00:02Z\",\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"model\":\"claude-one\",\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":20,\"cache_creation_input_tokens\":5,\"output_tokens\":2},\"content\":[{\"type\":\"tool_use\",\"id\":\"t2\",\"name\":\"Shell\",\"input\":{\"command\":\"true\"}}]}}\n"
            ),
        )
        .unwrap();

        let parsed = analyze_session(&session(&path, AgentKind::Claude), None).unwrap();
        assert_eq!(parsed.analytics.responses.len(), 1);
        assert_eq!(parsed.analytics.responses[0].usage.input_tokens, 30);
        assert_eq!(parsed.analytics.responses[0].usage.total_tokens, 37);
        assert_eq!(parsed.analytics.tools.len(), 2);
        assert_eq!(parsed.analytics.skills[0].name, "foo");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cursor_role_records_use_session_time_for_approximate_runs_and_tools() {
        let root = temp_dir("tendi-analytics-cursor");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session-1.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"do it\"}]}}\n",
                "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{\"path\":\"/tmp/skills/foo/SKILL.md\"}}]}}\n"
            ),
        )
        .unwrap();
        let mut cursor_session = session(&path, AgentKind::Cursor);
        cursor_session.started_at = Some("2026-08-01T01:00:00Z".to_string());
        let parsed = analyze_session(&cursor_session, None).unwrap();
        assert_eq!(parsed.analytics.tools.len(), 1);
        assert_eq!(parsed.analytics.tools[0].timestamp, "2026-08-01T01:00:00Z");
        assert_eq!(parsed.analytics.skills[0].name, "foo");
        assert_eq!(parsed.analytics.snapshot_runs(&parsed.state).len(), 1);
        assert!(parsed.analytics.snapshot_runs(&parsed.state)[0].completed);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cursor_analytics_uses_embedded_timestamps_for_each_turn() {
        let root = temp_dir("tendi-analytics-cursor-embedded-timestamp");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session-1.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<timestamp>Thursday, Aug 27, 2026, 11:59 PM (UTC+8)</timestamp>\\n<user_query>First</user_query>\"}]}}\n",
                "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{\"path\":\"/tmp/first\"}}]}}\n",
                "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<timestamp>Friday, Aug 28, 2026, 12:01 AM (UTC+8)</timestamp>\\n<user_query>Second</user_query>\"}]}}\n",
                "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{\"path\":\"/tmp/second\"}}]}}\n"
            ),
        )
        .unwrap();

        let parsed = analyze_session(&session(&path, AgentKind::Cursor), None).unwrap();

        assert_eq!(
            parsed
                .analytics
                .tools
                .iter()
                .map(|tool| tool.timestamp.as_str())
                .collect::<Vec<_>>(),
            vec!["2026-08-27T23:59:00+08:00", "2026-08-28T00:01:00+08:00"]
        );
        let runs = parsed.analytics.snapshot_runs(&parsed.state);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].start, "2026-08-27T23:59:00+08:00");
        assert_eq!(runs[0].end, "2026-08-27T23:59:00+08:00");
        assert_eq!(runs[1].start, "2026-08-28T00:01:00+08:00");
        assert_eq!(runs[1].end, "2026-08-28T00:01:00+08:00");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn append_parse_matches_full_parse() {
        let root = temp_dir("tendi-analytics-append");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-session-1.jsonl");
        fs::write(
            &path,
            "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":10,\"total_tokens\":10}}}}\n",
        )
        .unwrap();
        let session = session(&path, AgentKind::Codex);
        let first = analyze_session(&session, None).unwrap();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            file,
            "{{\"timestamp\":\"2026-08-01T01:00:01Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":25,\"total_tokens\":25}}}}}}}}"
        )
        .unwrap();
        let appended = analyze_session(&session, Some(&first)).unwrap();
        let full = analyze_session(&session, None).unwrap();
        assert_eq!(appended.analytics.responses, full.analytics.responses);
        assert_eq!(appended.state.previous_usage, full.state.previous_usage);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_parser_defers_partial_trailing_line_and_resumes_once() {
        let root = temp_dir("tendi-analytics-partial-append");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-session-1.jsonl");
        let first = "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":10,\"total_tokens\":10}}}}\n";
        let second = "{\"timestamp\":\"2026-08-01T01:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":25,\"total_tokens\":25}}}}\n";
        let split = second.len() / 2;
        fs::write(&path, format!("{first}{}", &second[..split])).unwrap();
        let session = session(&path, AgentKind::Codex);

        let partial = analyze_session(&session, None).unwrap();
        assert_eq!(partial.analytics.responses.len(), 1);
        assert_eq!(partial.file_size as usize, first.len());

        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        write!(file, "{}", &second[split..]).unwrap();
        let resumed = analyze_session(&session, Some(&partial)).unwrap();

        assert_eq!(resumed.analytics.responses.len(), 2);
        assert_eq!(resumed.analytics.responses[1].usage.total_tokens, 15);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_parser_accepts_valid_final_json_without_newline() {
        let root = temp_dir("tendi-analytics-final-line");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-session-1.jsonl");
        fs::write(
            &path,
            "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":10,\"total_tokens\":10}}}}",
        )
        .unwrap();

        let parsed = analyze_session(&session(&path, AgentKind::Codex), None).unwrap();
        assert_eq!(parsed.analytics.responses.len(), 1);
        assert_eq!(parsed.file_size, fs::metadata(&path).unwrap().len() as i64);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rewritten_larger_source_does_not_append_to_old_analytics() {
        let root = temp_dir("tendi-analytics-rewrite");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-session-1.jsonl");
        fs::write(
            &path,
            "{\"timestamp\":\"2026-08-01T01:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":10,\"total_tokens\":10}}}}\n",
        )
        .unwrap();
        let session = session(&path, AgentKind::Codex);
        let first = analyze_session(&session, None).unwrap();
        fs::write(
            &path,
            "{\"timestamp\":\"2026-08-02T01:00:00Z\",\"type\":\"event_msg\",\"padding\":\"rewritten-file-is-longer-than-the-original\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":50,\"total_tokens\":50}}}}\n",
        )
        .unwrap();

        let rewritten = analyze_session(&session, Some(&first)).unwrap();
        assert_eq!(rewritten.analytics.responses.len(), 1);
        assert_eq!(rewritten.analytics.responses[0].usage.total_tokens, 50);
        assert_eq!(
            rewritten.analytics.responses[0].timestamp,
            "2026-08-02T01:00:00Z"
        );
        fs::remove_dir_all(root).unwrap();
    }


    #[test]
    fn overview_keeps_model_attribution_and_mcp_servers_separate() {
        let timestamp = Local::now().to_rfc3339();
        let usage = AnalyticsTokenUsage {
            input_tokens: 8,
            output_tokens: 2,
            total_tokens: 10,
            ..AnalyticsTokenUsage::default()
        };
        let record = SessionAnalyticsRecord {
            analytics: SessionAnalytics {
                session_id: "overview-1".to_string(),
                agent: AgentKind::Codex,
                session_path: PathBuf::from("/tmp/overview-1.jsonl"),
                responses: vec![AnalyticsResponseUsage {
                    index: 1,
                    timestamp: timestamp.clone(),
                    model: "gpt-test".to_string(),
                    usage,
                    cumulative: usage,
                }],
                tools: vec![
                    AnalyticsToolCall {
                        timestamp: timestamp.clone(),
                        name: "search".to_string(),
                        server: "alpha".to_string(),
                    },
                    AnalyticsToolCall {
                        timestamp: timestamp.clone(),
                        name: "search".to_string(),
                        server: "beta".to_string(),
                    },
                ],
                skills: vec![AnalyticsSkillCall {
                    timestamp,
                    name: "debugging".to_string(),
                }],
                ..SessionAnalytics::default()
            },
            state: AnalyticsParserState::default(),
            file_mtime: 0,
            file_size: 0,
        };

        let projected_record = overview_record(&record);
        let projected = aggregate_overview_records(&[projected_record], 1, 1, Vec::new());
        let overview = aggregate_overview(std::slice::from_ref(&record), 1, 1, Vec::new());
        assert_eq!(
            serde_json::to_value(&overview.summary).unwrap(),
            serde_json::to_value(&projected.summary).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&overview.days).unwrap(),
            serde_json::to_value(&projected.days).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&overview.tools).unwrap(),
            serde_json::to_value(&projected.tools).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&overview.skills).unwrap(),
            serde_json::to_value(&projected.skills).unwrap()
        );
        assert_eq!(overview.summary.usage.total_tokens, 10);
        assert_eq!(overview.days[0].models[0].model, "gpt-test");
        assert_eq!(overview.days[0].tools.len(), 2);
        assert_eq!(
            overview.days[0]
                .tools
                .iter()
                .map(|tool| tool.calls)
                .sum::<u64>(),
            2
        );
        assert_eq!(overview.days[0].skills[0].name, "debugging");
        assert_eq!(overview.days[0].skills[0].calls, 1);
        assert_eq!(overview.tools.len(), 2);
        assert_eq!(overview.tools[0].name, "search");
        assert_ne!(overview.tools[0].server, overview.tools[1].server);
    }

    #[test]
    fn overview_tracks_daily_sessions_by_agent() {
        let timestamp = Local::now().to_rfc3339();
        let record = |id: &str, agent: AgentKind| SessionAnalyticsRecord {
            analytics: SessionAnalytics {
                session_id: id.to_string(),
                agent,
                session_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
                responses: vec![AnalyticsResponseUsage {
                    index: 1,
                    timestamp: timestamp.clone(),
                    model: String::new(),
                    usage: AnalyticsTokenUsage::default(),
                    cumulative: AnalyticsTokenUsage::default(),
                }],
                ..SessionAnalytics::default()
            },
            state: AnalyticsParserState::default(),
            file_mtime: 0,
            file_size: 0,
        };

        let overview = aggregate_overview(
            &[
                record("codex-session", AgentKind::Codex),
                record("claude-session", AgentKind::Claude),
            ],
            1,
            1,
            Vec::new(),
        );

        assert_eq!(overview.days[0].sessions, 2);
        assert_eq!(
            overview.days[0].sessions_by_agent.get(&AgentKind::Codex),
            Some(&1)
        );
        assert_eq!(
            overview.days[0].sessions_by_agent.get(&AgentKind::Claude),
            Some(&1)
        );
        let serialized = serde_json::to_value(&overview.days[0]).unwrap();
        assert_eq!(serialized["sessionsByAgent"]["codex"], 1);
        assert_eq!(serialized["sessionsByAgent"]["claude"], 1);
    }

    #[test]
    fn overview_index_uses_local_calendar_dates_across_midnight_offsets() {
        let local = Local::now().offset().fix();
        let first_local = local
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(2026, 8, 12)
                    .unwrap()
                    .and_hms_opt(23, 59, 0)
                    .unwrap(),
            )
            .single()
            .unwrap();
        let last_local = local
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(2026, 8, 13)
                    .unwrap()
                    .and_hms_opt(0, 1, 0)
                    .unwrap(),
            )
            .single()
            .unwrap();
        let record = SessionAnalytics {
            responses: vec![
                AnalyticsResponseUsage {
                    index: 1,
                    timestamp: first_local.to_rfc3339(),
                    model: String::new(),
                    usage: AnalyticsTokenUsage::default(),
                    cumulative: AnalyticsTokenUsage::default(),
                },
                AnalyticsResponseUsage {
                    index: 2,
                    timestamp: last_local.to_rfc3339(),
                    model: String::new(),
                    usage: AnalyticsTokenUsage::default(),
                    cumulative: AnalyticsTokenUsage::default(),
                },
            ],
            ..SessionAnalytics::default()
        };

        let index = record.overview_index(&AnalyticsParserState::default());

        assert_eq!(index.first.as_deref(), Some("2026-08-12"));
        assert_eq!(index.last.as_deref(), Some("2026-08-13"));
    }

    #[test]
    fn overview_hard_caps_extreme_history_to_one_year() {
        let overview = aggregate_overview(&[], 36_500, 30, Vec::new());

        assert_eq!(overview.days_requested, 365);
        assert_eq!(overview.days.len(), 365);
    }
}
