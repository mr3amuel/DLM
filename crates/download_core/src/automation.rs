//! Persistent wall-clock policy; admissions remain in Engine's existing queue.
use crate::{Download, Engine, Error, PauseOrigin, Result, ResumeCapability, Status, engine::Stop};
use chrono::{DateTime, Datelike, Duration as Days, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{SystemTime, UNIX_EPOCH},
};

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduleKind {
    #[default]
    Download,
    Bandwidth,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub kind: ScheduleKind,
    pub download_id: Option<String>,
    pub at_ms: Option<i64>,
    /// Monday bit 0 through Sunday bit 6; day refers to window start.
    pub weekdays: u8,
    pub start_minute: u16,
    pub end_minute: u16,
    pub timezone: String,
    pub bandwidth_limit: Option<u64>,
    pub last_occurrence: Option<i64>,
    /// Monotonic local calendar day prevents duplicate starts after timezone changes.
    #[serde(default)]
    pub last_local_day: Option<i32>,
    pub revision: u64,
    pub note: Option<String>,
}
impl ScheduleRule {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() || self.name.chars().count() > 80 {
            return Err(Error::Invalid(
                "Enter a schedule name of at most 80 characters".into(),
            ));
        }
        if self.id.len() > 80 || self.bandwidth_limit == Some(0) {
            return Err(Error::Invalid(
                "Use a positive bandwidth limit or Unlimited".into(),
            ));
        }
        if self.kind == ScheduleKind::Download
            && self.download_id.as_ref().is_none_or(|s| s.is_empty())
        {
            return Err(Error::Invalid("Select a download".into()));
        }
        if self.kind == ScheduleKind::Bandwidth
            && (self.at_ms.is_some() || self.download_id.is_some())
        {
            return Err(Error::Invalid("Bandwidth rules must be recurring".into()));
        }
        if let Some(at) = self.at_ms {
            if !(946684800000..4133980800000).contains(&at) {
                return Err(Error::Invalid(
                    "One-time date must be between 2000 and 2100".into(),
                ));
            }
        } else if self.weekdays == 0
            || self.weekdays > 127
            || self.start_minute >= 1440
            || self.end_minute >= 1440
            || self.start_minute == self.end_minute
        {
            return Err(Error::Invalid(
                "Select weekdays and distinct window start and end times".into(),
            ));
        }
        self.timezone.parse::<Tz>().map_err(|_| {
            Error::Invalid("Enter a valid IANA timezone, for example Asia/Tehran".into())
        })?;
        Ok(())
    }
    fn occurrence(&self, date: NaiveDate) -> Option<(i64, i64)> {
        if self.weekdays & (1 << date.weekday().num_days_from_monday()) == 0 {
            return None;
        }
        let tz = self.timezone.parse::<Tz>().ok()?;
        let end_date = if self.end_minute < self.start_minute {
            date.checked_add_signed(Days::days(1))?
        } else {
            date
        };
        let resolve = |d: NaiveDate, minute: u16, end: bool| {
            let value = d.and_hms_opt((minute / 60) as u32, (minute % 60) as u32, 0)?;
            match tz.from_local_datetime(&value) {
                LocalResult::Single(v) => Some(v.timestamp_millis()),
                LocalResult::Ambiguous(a, b) => {
                    Some(if end { a.max(b) } else { a.min(b) }.timestamp_millis())
                }
                LocalResult::None => None,
            }
        };
        Some((
            resolve(date, self.start_minute, false)?,
            resolve(end_date, self.end_minute, true)?,
        ))
    }
    fn local_day(&self, start: i64) -> Option<i32> {
        Some(
            DateTime::<Utc>::from_timestamp_millis(start)?
                .with_timezone(&self.timezone.parse::<Tz>().ok()?)
                .date_naive()
                .num_days_from_ce(),
        )
    }
    fn handled(&self, start: i64) -> bool {
        if self.at_ms.is_none()
            && let Some(last) = self.last_local_day
        {
            self.local_day(start).is_none_or(|day| day <= last)
        } else {
            self.last_occurrence.is_some_and(|last| start <= last)
        }
    }
    pub fn window(&self, now: i64) -> Option<(i64, i64)> {
        if !self.enabled {
            return None;
        }
        if let Some(at) = self.at_ms {
            return (now >= at).then_some((at, i64::MAX));
        }
        let tz = self.timezone.parse::<Tz>().ok()?;
        let date = DateTime::<Utc>::from_timestamp_millis(now)?
            .with_timezone(&tz)
            .date_naive();
        [date.checked_sub_signed(Days::days(1))?, date]
            .into_iter()
            .filter_map(|d| self.occurrence(d))
            .find(|(a, b)| *a <= now && now < *b)
    }
    pub fn next(&self, now: i64) -> Option<i64> {
        if !self.enabled {
            return None;
        }
        if let Some(at) = self.at_ms {
            return self
                .last_occurrence
                .is_none_or(|last| at > last)
                .then_some(at.max(now));
        }
        if let Some((start, _)) = self.window(now)
            && !self.handled(start)
        {
            return Some(now);
        }
        let tz = self.timezone.parse::<Tz>().ok()?;
        let date = DateTime::<Utc>::from_timestamp_millis(now)?
            .with_timezone(&tz)
            .date_naive();
        (0..=8)
            .filter_map(|n| date.checked_add_signed(Days::days(n)))
            .filter_map(|d| self.occurrence(d))
            .map(|(a, _)| a)
            .find(|a| *a > now && !self.handled(*a))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Ownership {
    rule_id: String,
    start: i64,
    end: i64,
    closed: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MissedOccurrence {
    start: i64,
    end: i64,
    dismissed: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AutomationState {
    pub rules: BTreeMap<String, ScheduleRule>,
    pub holds: BTreeSet<String>,
    owners: BTreeMap<String, Ownership>,
    pub bandwidth_override: Option<String>,
    #[serde(default)]
    pub system_timezone: Option<String>,
    #[serde(default)]
    pub(crate) started_once: BTreeSet<String>,
    #[serde(default)]
    missed: BTreeMap<String, MissedOccurrence>,
    #[serde(default)]
    pub(crate) last_seen_ms: Option<i64>,
}
impl AutomationState {
    /// Shell supplies the zone identifier; Rust owns all calendar transitions.
    pub(crate) fn follow_timezone(&mut self, zone: &str, now: i64) -> Result<()> {
        zone.parse::<Tz>()
            .map_err(|_| Error::Invalid("System timezone is not recognized".into()))?;
        for rule in self.rules.values_mut().filter(|r| r.at_ms.is_none()) {
            if rule.last_local_day.is_none() {
                rule.last_local_day = rule.last_occurrence.and_then(|at| rule.local_day(at));
            }
            rule.timezone = zone.into();
            if let Some(owner) = rule
                .download_id
                .as_ref()
                .and_then(|id| self.owners.get_mut(id))
                && !owner.closed
                && let Some((start, end)) = rule.window(now)
                && rule.local_day(start) == rule.last_local_day
            {
                // Continue the same logical day, with its new wall-clock boundaries.
                owner.start = start;
                owner.end = end;
                rule.last_occurrence = Some(start);
            }
        }
        self.system_timezone = Some(zone.into());
        Ok(())
    }

    fn reclaim_finished(&mut self, downloads: &BTreeMap<String, Download>, keep: Option<&str>) {
        self.rules.retain(|id, r| {
            if keep == Some(id.as_str()) || r.kind == ScheduleKind::Bandwidth {
                return true;
            }
            let Some(d) = r.download_id.as_ref().and_then(|id| downloads.get(id)) else {
                return false;
            };
            if matches!(d.status, Status::Completed | Status::Cancelled) {
                return false;
            }
            !(self.started_once.contains(id)
                && (d.status == Status::Failed
                    || (d.status == Status::Paused && d.pause_origin == Some(PauseOrigin::User))))
        });
    }
    pub(crate) fn record_started(&mut self, download_id: &str) {
        for r in self
            .rules
            .values()
            .filter(|r| r.download_id.as_deref() == Some(download_id) && r.at_ms.is_some())
        {
            self.started_once.insert(r.id.clone());
            self.missed.remove(&r.id);
        }
    }
    fn mark_missed(&mut self, id: &str, start: i64, end: i64) {
        if let Some(r) = self.rules.get_mut(id) {
            r.last_occurrence = Some(start);
            r.last_local_day = if r.at_ms.is_none() {
                r.local_day(start)
            } else {
                None
            };
            r.note = Some("Scheduled time was missed".into());
            if let Some(id) = &r.download_id {
                self.owners.remove(id);
            }
        }
        self.missed.insert(
            id.into(),
            MissedOccurrence {
                start,
                end,
                dismissed: false,
            },
        );
    }
    pub(crate) fn detect_missed_on_open(
        &mut self,
        now: i64,
        downloads: &BTreeMap<String, Download>,
    ) {
        let since = self
            .last_seen_ms
            .unwrap_or(now.saturating_sub(8 * 86_400_000));
        for r in self.rules.values().cloned().collect::<Vec<_>>() {
            if !r.enabled || r.kind != ScheduleKind::Download || self.started_once.contains(&r.id) {
                continue;
            }
            let Some(d) = r.download_id.as_ref().and_then(|id| downloads.get(id)) else {
                continue;
            };
            if self.holds.contains(&d.id)
                || d.pause_origin == Some(PauseOrigin::User)
                || matches!(d.status, Status::Completed | Status::Cancelled)
            {
                continue;
            }
            let missed = if let Some(at) = r.at_ms {
                (at < now && !self.missed.contains_key(&r.id)).then_some((at, i64::MAX))
            } else {
                r.window(now)
                    .filter(|(start, _)| *start < now && !r.handled(*start))
                    .or_else(|| {
                        let zone = r.timezone.parse::<Tz>().ok();
                        zone.and_then(|tz| {
                            DateTime::<Utc>::from_timestamp_millis(now)
                                .map(|n| n.with_timezone(&tz).date_naive())
                        })
                        .and_then(|date| {
                            (0..=7)
                                .filter_map(|n| date.checked_sub_signed(Days::days(n)))
                                .filter_map(|date| r.occurrence(date))
                                .find(|(start, _)| {
                                    *start > since && *start < now && !r.handled(*start)
                                })
                        })
                    })
            };
            if let Some((start, end)) = missed {
                self.mark_missed(&r.id, start, end);
            }
        }
        self.last_seen_ms = Some(now);
    }
    pub fn validate(&self) -> Result<()> {
        if self.rules.len() > 256
            || self.holds.len() > 10000
            || self.owners.len() > 10000
            || self.started_once.len() > 256
            || self.missed.len() > 256
        {
            return Err(Error::Storage(
                "Automation state exceeds safety bounds".into(),
            ));
        }
        if let Some(zone) = &self.system_timezone {
            zone.parse::<Tz>()
                .map_err(|_| Error::Storage("Invalid persisted system timezone".into()))?;
        }
        if self.started_once.iter().any(|id| {
            self.rules
                .get(id)
                .is_none_or(|r| r.kind != ScheduleKind::Download || r.at_ms.is_none())
        }) || self.missed.iter().any(|(id, m)| {
            m.start >= m.end
                || self
                    .rules
                    .get(id)
                    .is_none_or(|r| r.kind != ScheduleKind::Download)
        }) {
            return Err(Error::Storage(
                "Invalid persisted schedule lifecycle".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        for (id, r) in &self.rules {
            r.validate()?;
            if *id != r.id {
                return Err(Error::Storage("Schedule ID mismatch".into()));
            }
            if r.kind == ScheduleKind::Download && !ids.insert(r.download_id.clone()) {
                return Err(Error::Storage(
                    "Only one schedule per download is supported".into(),
                ));
            }
        }
        for (id, owner) in &self.owners {
            if owner.start >= owner.end
                || self.rules.get(&owner.rule_id).is_none_or(|r| {
                    r.kind != ScheduleKind::Download
                        || r.download_id.as_ref() != Some(id)
                        || r.last_occurrence != Some(owner.start)
                })
            {
                return Err(Error::Storage(
                    "Invalid persisted schedule ownership".into(),
                ));
            }
        }
        Ok(())
    }
    pub fn owns(&self, id: &str) -> bool {
        self.owners.contains_key(id)
    }
    pub fn permitted(&self, now: i64) -> BTreeMap<String, bool> {
        self.rules
            .values()
            .filter(|r| r.kind == ScheduleKind::Download && r.enabled)
            .filter_map(|r| {
                let id = r.download_id.clone()?;
                let allowed = r.window(now).is_some_and(|(start, _)| {
                    self.owners
                        .get(&id)
                        .is_some_and(|o| o.rule_id == r.id && o.start == start && !o.closed)
                });
                Some((id, allowed))
            })
            .collect()
    }
    pub fn bandwidth_key(&self, now: i64) -> String {
        self.rules
            .values()
            .filter(|r| r.kind == ScheduleKind::Bandwidth)
            .filter_map(|r| {
                r.window(now)
                    .map(|(a, b)| format!("{}:{}:{a}:{b}", r.id, r.revision))
            })
            .collect::<Vec<_>>()
            .join("|")
    }
    pub fn effective_limit(&self, now: i64, manual: Option<u64>) -> Option<u64> {
        let key = self.bandwidth_key(now);
        if key.is_empty() || self.bandwidth_override.as_ref() == Some(&key) {
            return manual;
        }
        self.rules
            .values()
            .filter(|r| r.kind == ScheduleKind::Bandwidth && r.window(now).is_some())
            .filter_map(|r| r.bandwidth_limit)
            .min()
    }
}
#[derive(Debug, Clone)]
pub struct ScheduleView {
    pub rule: ScheduleRule,
    pub next_run_ms: Option<i64>,
    pub active: bool,
    pub held: bool,
    pub status: String,
    pub pending: bool,
    pub missed_at_ms: Option<i64>,
    pub notify_missed: bool,
}
#[derive(Debug, Clone)]
pub struct SchedulerView {
    pub rules: Vec<ScheduleView>,
    pub manual_limit: Option<u64>,
    pub effective_limit: Option<u64>,
    pub manual_override: bool,
    pub error: Option<String>,
}
/// Calendar date entered by the user. Resolve in Rust at save time, never in Dart.
#[derive(Debug, Clone, Copy)]
pub struct LocalScheduleDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}
impl Engine {
    pub async fn set_system_timezone(&self, zone: &str) -> Result<()> {
        let _lock = self.inner.commands.lock().await;
        self.ensure_open()?;
        let mut state = self.inner.automation.lock().await.clone();
        if state.system_timezone.as_deref() == Some(zone) {
            return Ok(());
        }
        state.follow_timezone(zone, self.inner.clock.now_ms())?;
        self.commit_automation(state, vec![], None).await?;
        self.refresh_automation_locked().await;
        Ok(())
    }

    pub async fn scheduler_view(&self) -> SchedulerView {
        let state = self.inner.automation.lock().await.clone();
        let now = self.inner.clock.now_ms();
        let downloads: BTreeMap<_, _> = {
            let map = self.inner.downloads.lock().await;
            state
                .rules
                .values()
                .filter_map(|r| r.download_id.as_ref())
                .filter_map(|id| map.get(id).map(|d| (id.clone(), d.clone())))
                .collect()
        };
        let manual = self.global_bandwidth_limit().await;
        let mut rules: Vec<ScheduleView> = state
            .rules
            .values()
            .map(|r| {
                let d = r.download_id.as_ref().and_then(|id| downloads.get(id));
                let held = d.is_some_and(|d| {
                    !matches!(d.status, Status::Completed | Status::Cancelled)
                        && (state.holds.contains(&d.id)
                            || d.pause_origin == Some(PauseOrigin::User))
                });
                let active = r.window(now).is_some();
                let missed = state.missed.get(&r.id);
                let status = if !r.enabled {
                    "Disabled"
                } else if held {
                    "Paused — edit the schedule to choose a new start"
                } else if missed.is_some() {
                    "Missed schedule — choose Start Now or a new time"
                } else if r.kind == ScheduleKind::Download && d.is_none() {
                    "Download unavailable"
                } else if d
                    .is_some_and(|d| matches!(d.status, Status::Cancelled | Status::Completed))
                {
                    "Download finished or cancelled"
                } else if d.is_some_and(|d| d.status == Status::Failed) {
                    "Download needs attention"
                } else if r
                    .note
                    .as_ref()
                    .is_some_and(|n| n.contains("cannot safely pause"))
                {
                    r.note.as_deref().unwrap()
                } else if d.is_some_and(|d| d.status == Status::Running) {
                    if state.owners.contains_key(&d.unwrap().id) {
                        "Downloading"
                    } else {
                        "Manual transfer continues"
                    }
                } else if d.is_some_and(|d| d.status == Status::Queued) {
                    "Queued"
                } else if r.note.as_deref()
                    == Some("Missed one-time start; start manually or choose a new time")
                {
                    r.note.as_deref().unwrap()
                } else if r.window(now).is_some_and(|(start, _)| r.handled(start))
                    && state
                        .owners
                        .get(r.download_id.as_deref().unwrap_or(""))
                        .is_none_or(|o| o.closed)
                {
                    "Waiting for next scheduled window"
                } else {
                    r.note.as_deref().unwrap_or(if active {
                        "Window active"
                    } else {
                        "Waiting for schedule"
                    })
                };
                ScheduleView {
                    rule: r.clone(),
                    next_run_ms: if (r.at_ms.is_some() && missed.is_some())
                        || held
                        || (r.kind == ScheduleKind::Download && d.is_none())
                        || d.is_some_and(|d| {
                            matches!(
                                d.status,
                                Status::Completed
                                    | Status::Cancelled
                                    | Status::Failed
                                    | Status::Running
                                    | Status::Finalizing
                                    | Status::Queued
                            )
                        }) {
                        None
                    } else if r.kind == ScheduleKind::Bandwidth {
                        r.window(now).map(|(_, end)| end).or_else(|| r.next(now))
                    } else {
                        r.next(now)
                    },
                    active,
                    held,
                    status: status.into(),
                    pending: r.kind == ScheduleKind::Bandwidth
                        || (!state.started_once.contains(&r.id)
                            && d.is_some_and(|d| {
                                !matches!(
                                    d.status,
                                    Status::Completed
                                        | Status::Cancelled
                                        | Status::Running
                                        | Status::Finalizing
                                )
                            })),
                    missed_at_ms: missed.map(|m| m.start),
                    notify_missed: missed.is_some_and(|m| !m.dismissed),
                }
            })
            .collect();
        rules.sort_by(|a, b| {
            a.next_run_ms
                .unwrap_or(i64::MAX)
                .cmp(&b.next_run_ms.unwrap_or(i64::MAX))
                .then_with(|| a.rule.id.cmp(&b.rule.id))
        });
        SchedulerView {
            rules,
            manual_limit: manual,
            effective_limit: *self.inner.automation_effective.lock().await,
            manual_override: !state.bandwidth_key(now).is_empty()
                && state.bandwidth_override.as_ref() == Some(&state.bandwidth_key(now)),
            error: self.inner.automation_error.lock().await.clone(),
        }
    }
    pub(crate) async fn commit_manual_start(&self, d: Download) -> Result<()> {
        let mut state = self.inner.automation.lock().await.clone();
        let one_time = state
            .rules
            .values()
            .find(|r| r.download_id.as_deref() == Some(&d.id) && r.at_ms.is_some())
            .map(|r| r.id.clone());
        if let Some(rule) = one_time {
            state.rules.remove(&rule);
            state.owners.remove(&d.id);
        } else if state
            .rules
            .values()
            .any(|r| r.download_id.as_deref() == Some(&d.id))
        {
            state.holds.insert(d.id.clone());
            state.owners.remove(&d.id);
        }
        self.commit_automation(state, vec![d], None).await
    }
    pub async fn dismiss_missed_schedules(&self, keys: &[(String, i64)]) -> Result<()> {
        let _lock = self.inner.commands.lock().await;
        self.ensure_open()?;
        let mut state = self.inner.automation.lock().await.clone();
        for (id, at) in keys {
            if let Some(m) = state.missed.get_mut(id)
                && m.start == *at
            {
                m.dismissed = true;
            }
        }
        self.commit_automation(state, vec![], None).await
    }
    pub(crate) async fn manual_override_locked(&self, id: &str) -> Result<()> {
        let mut state = self.inner.automation.lock().await.clone();
        if state.owners.contains_key(id)
            || state
                .rules
                .values()
                .any(|r| r.download_id.as_deref() == Some(id))
        {
            state.holds.insert(id.into());
            state.owners.remove(id);
            self.commit_automation(state, vec![], None).await?;
        }
        Ok(())
    }
    pub(crate) async fn commit_automation(
        &self,
        mut state: AutomationState,
        records: Vec<Download>,
        policy: Option<crate::model::PolicySettings>,
    ) -> Result<()> {
        let targets: BTreeSet<_> = state
            .rules
            .values()
            .filter_map(|r| r.download_id.clone())
            .collect();
        state.holds.retain(|id| targets.contains(id));
        state.started_once.retain(|id| state.rules.contains_key(id));
        state.missed.retain(|id, _| state.rules.contains_key(id));
        state.owners.retain(|id, owner| {
            state
                .rules
                .get(&owner.rule_id)
                .is_some_and(|r| r.download_id.as_ref() == Some(id))
        });
        self.inner
            .storage
            .automation(Some((state.clone(), records.clone(), policy)))
            .await?;
        *self.inner.automation.lock().await = state;
        for d in records {
            if !self.inner.downloads.lock().await.contains_key(&d.id) {
                self.inner.bandwidth.set_local(&d.id, d.bandwidth_limit);
            }
            self.publish(&d).await;
        }
        Ok(())
    }
    pub async fn save_schedule(&self, rule: ScheduleRule) -> Result<String> {
        self.save_schedule_local(rule, None).await
    }
    pub async fn save_schedule_local(
        &self,
        rule: ScheduleRule,
        date: Option<LocalScheduleDate>,
    ) -> Result<String> {
        self.save_schedule_with_consent(rule, date, true).await
    }
    pub async fn save_schedule_with_consent(
        &self,
        mut rule: ScheduleRule,
        date: Option<LocalScheduleDate>,
        _authorize: bool,
    ) -> Result<String> {
        // Saving is the explicit user instruction. A later Pause remains authoritative.
        let authorize = rule.kind == ScheduleKind::Download;
        let _lock = self.inner.commands.lock().await;
        self.ensure_open()?;
        let mut state = self.inner.automation.lock().await.clone();
        state.reclaim_finished(&*self.inner.downloads.lock().await, Some(&rule.id));
        let fallback = if let Some(id) = &rule.download_id {
            self.get(id).await?.filename
        } else {
            "Bandwidth rule".into()
        };
        Self::normalize_rule(&mut rule, date, &state, &fallback)?;
        let mut records = vec![];
        if rule.id.is_empty() {
            rule.id = uuid::Uuid::new_v4().to_string();
        }
        if !state.rules.contains_key(&rule.id) && state.rules.len() >= 256 {
            return Err(Error::Invalid("Schedule limit reached (256)".into()));
        }
        if let Some(id) = &rule.download_id {
            self.guard_recovery(id)?;
            let old_once = state
                .rules
                .values()
                .find(|r| {
                    r.download_id.as_ref() == Some(id)
                        && r.at_ms.is_some()
                        && state.started_once.contains(&r.id)
                })
                .map(|r| r.id.clone());
            if let Some(old) = old_once
                && !state.rules.contains_key(&rule.id)
            {
                state.rules.remove(&old);
                state.owners.remove(id);
            }
            rule.enabled = true;
            let mut d = self.get(id).await?;
            if matches!(d.status, Status::Running | Status::Finalizing) {
                return Err(Error::Busy(
                    "Pause this download before scheduling; its current transfer will continue"
                        .into(),
                ));
            }
            if matches!(d.status, Status::Completed | Status::Cancelled) {
                return Err(Error::Invalid("Choose an unfinished download".into()));
            }
            if state
                .rules
                .values()
                .any(|r| r.id != rule.id && r.download_id.as_ref() == Some(id))
            {
                return Err(Error::Invalid(
                    "Only one schedule per download is supported".into(),
                ));
            }
            if authorize {
                state.holds.remove(id);
                state.owners.remove(id);
                d.pause_origin = Some(PauseOrigin::Scheduler);
                if d.status == Status::Failed {
                    d.status = Status::Paused;
                }
            }
            if d.status == Status::Queued {
                // Defer this queued record in the SAME transaction that installs its gate.
                d.status = Status::Paused;
                d.queue_position = None;
                if d.pause_origin != Some(PauseOrigin::User) {
                    d.pause_origin = Some(PauseOrigin::Scheduler);
                }
            }
            records.push(d);
        }
        if let Some(old) = state.rules.get(&rule.id).cloned() {
            if old.kind != rule.kind || old.download_id != rule.download_id {
                return Err(Error::Invalid("Schedule target cannot be changed".into()));
            }
            rule.last_occurrence = if rule.kind == old.kind && rule.download_id == old.download_id {
                old.last_occurrence
            } else {
                None
            };
            rule.last_local_day = if rule.kind == old.kind && rule.download_id == old.download_id {
                old.last_local_day
            } else {
                None
            };
            rule.revision = old
                .revision
                .checked_add(1)
                .ok_or_else(|| Error::Invalid("Schedule revision exhausted".into()))?;
            if let Some(id) = &old.download_id {
                state.owners.remove(id);
            }
        } else {
            rule.last_occurrence = None;
            rule.last_local_day = None;
            rule.revision = 1;
        }
        if authorize {
            state.missed.remove(&rule.id);
            state.started_once.remove(&rule.id);
            rule.last_occurrence = None;
            rule.last_local_day = None;
        }
        rule.note = None;
        state.last_seen_ms = Some(self.inner.clock.now_ms());
        let id = rule.id.clone();
        state.rules.insert(id.clone(), rule);
        self.commit_automation(state, records, None).await?;
        self.refresh_automation_locked().await;
        Ok(id)
    }
    fn normalize_rule(
        rule: &mut ScheduleRule,
        date: Option<LocalScheduleDate>,
        state: &AutomationState,
        fallback: &str,
    ) -> Result<()> {
        rule.timezone = state
            .system_timezone
            .clone()
            .unwrap_or_else(|| "UTC".into());
        if let Some(date) = date {
            if rule.kind != ScheduleKind::Download || rule.start_minute >= 1440 {
                return Err(Error::Invalid("Select a valid download start time".into()));
            }
            let local = NaiveDate::from_ymd_opt(date.year, date.month, date.day)
                .and_then(|d| {
                    d.and_hms_opt(
                        (rule.start_minute / 60).into(),
                        (rule.start_minute % 60).into(),
                        0,
                    )
                })
                .ok_or_else(|| Error::Invalid("Select a valid date".into()))?;
            let zone: Tz = rule
                .timezone
                .parse()
                .map_err(|_| Error::Invalid("System timezone is not recognized".into()))?;
            rule.at_ms = Some(match zone.from_local_datetime(&local) {
                LocalResult::Single(at) => at.timestamp_millis(),
                LocalResult::Ambiguous(a, b) => a.min(b).timestamp_millis(),
                LocalResult::None => return Err(Error::Invalid("This local time does not exist because of daylight saving. Choose another time.".into())),
            });
        }
        if rule.name.trim().is_empty() {
            rule.name = fallback.chars().take(80).collect();
        }
        rule.validate()?;
        Ok(())
    }
    /// Publish neither record nor queue eligibility before the single SQLite commit.
    pub async fn create_scheduled_download(
        &self,
        url: String,
        directory: std::path::PathBuf,
        filename: String,
        config: crate::TransferConfig,
        rule: ScheduleRule,
        date: Option<LocalScheduleDate>,
    ) -> Result<Download> {
        self.create_scheduled_with_settings(
            url,
            directory,
            filename,
            crate::CreationSettings {
                transfer: config,
                bandwidth_limit: None,
                ..Default::default()
            },
            rule,
            date,
        )
        .await
    }
    pub async fn create_scheduled_with_settings(
        &self,
        url: String,
        directory: std::path::PathBuf,
        filename: String,
        settings: crate::CreationSettings,
        mut rule: ScheduleRule,
        date: Option<LocalScheduleDate>,
    ) -> Result<Download> {
        let _lock = self.inner.commands.lock().await;
        self.ensure_open()?;
        settings.transfer.validate()?;
        crate::engine::validate_bandwidth(settings.bandwidth_limit)?;
        let mut state = self.inner.automation.lock().await.clone();
        state.reclaim_finished(&*self.inner.downloads.lock().await, None);
        if rule.kind != ScheduleKind::Download || !rule.id.is_empty() {
            return Err(Error::Invalid(
                "A new download requires a new download schedule".into(),
            ));
        }
        if state.rules.len() >= 256 {
            return Err(Error::Invalid("Schedule limit reached (256)".into()));
        }
        rule.download_id = Some("pending".into());
        Self::normalize_rule(&mut rule, date, &state, &filename)?;
        let (directory, filename, category) = self
            .prepare_creation(&url, directory, filename, &settings)
            .await?;
        let mut d = self.create_record_locked(url, directory, filename).await?;
        d.category_id = Some(category);
        d.category_manual = settings.category_id.is_some();
        d.content_type = settings.content_type_hint;
        d.config = settings.transfer;
        d.bandwidth_limit = settings.bandwidth_limit;
        d.pause_origin = Some(PauseOrigin::Scheduler);
        rule.id = uuid::Uuid::new_v4().to_string();
        rule.download_id = Some(d.id.clone());
        rule.last_occurrence = None;
        rule.last_local_day = None;
        rule.revision = 1;
        rule.note = None;
        state.rules.insert(rule.id.clone(), rule);
        state.last_seen_ms = Some(self.inner.clock.now_ms());
        if let Err(error) = self.commit_automation(state, vec![d.clone()], None).await {
            let _ = tokio::fs::remove_file(&d.partial_path).await;
            return Err(error);
        }
        // Normal supervisor may admit only after this commit, when the window permits.
        Ok(d)
    }
    async fn release_rule(&self, state: &mut AutomationState, rule: &ScheduleRule) -> Result<()> {
        if let Some(id) = &rule.download_id {
            self.guard_recovery(id)?;
            if !state.holds.contains(id)
                && self.get(id).await.is_ok_and(|d| d.status == Status::Queued)
            {
                self.stop_locked(id, Stop::Pause).await?;
                let mut d = self.get(id).await?;
                d.pause_origin = Some(PauseOrigin::Scheduler);
                self.persist(&d).await?;
            }
            state.owners.remove(id);
        }
        Ok(())
    }
    pub async fn set_schedule_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let _lock = self.inner.commands.lock().await;
        self.ensure_open()?;
        let mut state = self.inner.automation.lock().await.clone();
        let old = state
            .rules
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound("Schedule".into()))?;
        if !enabled {
            self.release_rule(&mut state, &old).await?;
        }
        let r = state.rules.get_mut(id).unwrap();
        r.enabled = enabled;
        r.revision = r.revision.saturating_add(1);
        // Enabling does not erase the persisted reason an occurrence was consumed.
        self.commit_automation(state, vec![], None).await?;
        self.refresh_automation_locked().await;
        Ok(())
    }
    pub async fn delete_schedule(&self, id: &str) -> Result<()> {
        let _lock = self.inner.commands.lock().await;
        self.ensure_open()?;
        let mut state = self.inner.automation.lock().await.clone();
        let r = state
            .rules
            .remove(id)
            .ok_or_else(|| Error::NotFound("Schedule".into()))?;
        self.release_rule(&mut state, &r).await?;
        self.commit_automation(state, vec![], None).await?;
        self.refresh_automation_locked().await;
        Ok(())
    }
    /// Explicit user rearm, never called implicitly by a timer or ordinary edit.
    pub async fn allow_schedule(&self, id: &str) -> Result<()> {
        let _lock = self.inner.commands.lock().await;
        self.ensure_open()?;
        let mut state = self.inner.automation.lock().await.clone();
        let r = state
            .rules
            .get_mut(id)
            .ok_or_else(|| Error::NotFound("Schedule".into()))?;
        let download_id = r
            .download_id
            .clone()
            .ok_or_else(|| Error::Invalid("Not a download schedule".into()))?;
        self.guard_recovery(&download_id)?;
        let mut d = self.get(&download_id).await?;
        if matches!(
            d.status,
            Status::Completed | Status::Cancelled | Status::Running | Status::Finalizing
        ) {
            return Err(Error::Busy(
                "Pause or finish manual work before rearming".into(),
            ));
        }
        r.last_occurrence = None;
        r.last_local_day = None;
        r.note = None;
        state.holds.remove(&download_id);
        state.owners.remove(&download_id);
        d.pause_origin = Some(PauseOrigin::Scheduler);
        if d.status == Status::Failed {
            d.status = Status::Paused;
        }
        self.commit_automation(state, vec![d], None).await?;
        self.refresh_automation_locked().await;
        Ok(())
    }
    async fn refresh_automation_locked(&self) {
        let result = self.automation_tick_locked().await;
        let result = match result {
            Ok(()) => self.schedule_locked().await,
            Err(e) => Err(e),
        };
        *self.inner.automation_error.lock().await = result.err().map(|e| e.to_string());
    }
    pub async fn tick_automation(&self) -> Result<()> {
        let _lock = self.inner.commands.lock().await;
        self.ensure_open()?;
        let result = self.automation_tick_locked().await;
        let result = match result {
            Ok(()) => self.schedule_locked().await,
            Err(e) => Err(e),
        };
        *self.inner.automation_error.lock().await = result.as_ref().err().map(|e| e.to_string());
        result
    }
    pub(crate) async fn automation_tick_locked(&self) -> Result<()> {
        let now = self.inner.clock.now_ms();
        let mut state = self.inner.automation.lock().await.clone();
        let current_key = state.bandwidth_key(now);
        if state
            .bandwidth_override
            .as_ref()
            .is_some_and(|key| key != &current_key)
        {
            state.bandwidth_override = None;
        }
        // Persist closed ownership BEFORE asking writers to stop. A crash cannot
        // replay the same closed window after a backward clock adjustment.
        for (id, owner) in state.owners.clone() {
            if owner.closed
                || state
                    .rules
                    .get(&owner.rule_id)
                    .and_then(|r| r.window(now))
                    .is_some_and(|(start, end)| start == owner.start && end == owner.end)
            {
                continue;
            }
            state.owners.get_mut(&id).unwrap().closed = true;
            self.commit_automation(state.clone(), vec![], None).await?;
            if let Ok(d) = self.get(&id).await {
                let safe = d.status == Status::Queued
                    || (d.status == Status::Running
                        && d.resume.capability == ResumeCapability::Supported);
                if safe {
                    self.stop_locked(&id, Stop::Pause).await?;
                    let mut d = self.get(&id).await?;
                    if d.status == Status::Paused {
                        d.pause_origin = Some(PauseOrigin::Scheduler);
                        self.persist(&d).await?;
                    }
                }
                if let Some(r) = state.rules.get_mut(&owner.rule_id) {
                    r.note = Some(
                        if safe {
                            "Paused until the next scheduled window"
                        } else if d.status == Status::Running {
                            "Download continues because it cannot safely pause"
                        } else {
                            "Window closed — waiting for next window"
                        }
                        .into(),
                    );
                }
            }
        }
        for mut rule in state.rules.values().cloned().collect::<Vec<_>>() {
            if !rule.enabled || rule.kind != ScheduleKind::Download {
                continue;
            }
            let Some(id) = rule.download_id.clone() else {
                continue;
            };
            let Ok(d) = self.get(&id).await else {
                continue;
            };
            if state.holds.contains(&id) || d.pause_origin == Some(PauseOrigin::User) {
                continue;
            }
            if state.missed.get(&rule.id).is_some_and(|m| {
                rule.at_ms.is_some() || rule.window(now).is_none_or(|(start, _)| start <= m.start)
            }) {
                continue;
            }
            if let Some(at) = rule.at_ms
                && rule.last_occurrence.is_none_or(|last| at > last)
                && now > at.saturating_add(15 * 60 * 1000)
            {
                state.mark_missed(&rule.id, at, i64::MAX);
                continue;
            }
            let Some((start, end)) = rule.window(now) else {
                continue;
            };
            let continuation = state
                .owners
                .get(&id)
                .is_some_and(|o| !o.closed && o.rule_id == rule.id && o.start == start);
            if rule.handled(start) && !continuation {
                continue;
            }
            if continuation && d.status == Status::Queued {
                continue;
            }
            if !matches!(d.status, Status::Ready | Status::Paused | Status::Queued) {
                continue;
            }
            if d.durable_bytes > 0 && d.etag.is_none() {
                rule.note = Some("Resume safety unavailable — manual action required".into());
                state.rules.insert(rule.id.clone(), rule);
                continue;
            }
            if self.guard_recovery(&id).is_err() {
                continue;
            }
            let mut queued = if d.status == Status::Queued {
                d
            } else {
                match self.prepare_enqueue_locked(&id).await {
                    Ok(d) => d,
                    Err(Error::Busy(_)) => {
                        rule.note = Some("Waiting for queue capacity or manual operation".into());
                        state.rules.insert(rule.id.clone(), rule);
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            };
            state.missed.remove(&rule.id);
            queued.automation_strict_resume = true;
            queued.pause_origin = None;
            rule.last_occurrence = Some(start);
            rule.last_local_day = if rule.at_ms.is_none() {
                rule.local_day(start)
            } else {
                None
            };
            rule.note = Some("Waiting to start".into());
            state.owners.insert(
                id,
                Ownership {
                    rule_id: rule.id.clone(),
                    start,
                    end,
                    closed: false,
                },
            );
            state.rules.insert(rule.id.clone(), rule);
            self.commit_automation(state.clone(), vec![queued], None)
                .await?;
        }
        if state != *self.inner.automation.lock().await {
            self.commit_automation(state.clone(), vec![], None).await?;
        }
        let manual = self.global_bandwidth_limit().await;
        let effective = state.effective_limit(now, manual);
        // set_global preserves/clamps credit; avoid resetting on each tick.
        let mut map = self.inner.downloads.lock().await;
        let mut changed = false;
        for d in map.values_mut() {
            let rule = state.rules.values().find(|r| {
                r.enabled
                    && r.download_id.as_deref() == Some(&d.id)
                    && !matches!(d.status, Status::Completed | Status::Cancelled)
            });
            let label = rule.map(|r| {
                if state.holds.contains(&d.id) || d.pause_origin == Some(PauseOrigin::User) {
                    "Paused — edit the schedule to choose a new start".into()
                } else if d.status == Status::Running {
                    r.note
                        .clone()
                        .filter(|n| n.contains("cannot safely pause"))
                        .unwrap_or_else(|| {
                            if state.owners.contains_key(&d.id) {
                                "Scheduled download".into()
                            } else {
                                "Manual transfer continues".into()
                            }
                        })
                } else if d.status == Status::Failed {
                    "Download needs attention".into()
                } else if r.window(now).is_none() {
                    "Waiting for schedule".into()
                } else {
                    r.note.clone().unwrap_or_else(|| "Window active".into())
                }
            });
            if d.schedule_state != label {
                d.schedule_state = label;
                changed = true;
            }
        }
        // Cached effective policy belongs to the engine, not persistent manual settings.
        let mut previous = self.inner.automation_effective.lock().await;
        if *previous != effective {
            self.inner.bandwidth.set_global(effective);
            *previous = effective;
            changed = true;
        }
        for d in map.values_mut() {
            d.bandwidth_limited = self.inner.bandwidth.is_capped(&d.id);
        }
        if changed {
            self.inner
                .events
                .send_replace(map.values().cloned().collect());
        }
        Ok(())
    }
}
