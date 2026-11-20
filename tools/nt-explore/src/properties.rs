use crate::model::{Action, Property, State, Violation};

#[derive(Clone, Debug)]
struct Page {
    kind: String,
    results_query: Option<String>,
    scroll_y: f64,
    video_id: Option<String>,
}

impl From<&State> for Page {
    fn from(state: &State) -> Self {
        Self {
            kind: state.page.clone(),
            results_query: state.results_query.clone(),
            scroll_y: state.scroll_y,
            video_id: state.video.as_ref().map(|video| video.id.clone()),
        }
    }
}

#[derive(Debug)]
enum Expectation {
    Restore { page: Page, depth: usize },
    Watch { prior_id: Option<String> },
    NoMenu,
    Search(String),
    Loaded,
}

impl Expectation {
    fn property(&self) -> Property {
        match self {
            Self::Restore { .. } => Property::BackRestoration,
            Self::Watch { .. } => Property::WatchFirstFrame,
            Self::NoMenu => Property::EscapeDismissesMenu,
            Self::Search(_) => Property::SearchResults,
            Self::Loaded => Property::LoadingEnds,
        }
    }

    fn matches(&self, state: &State) -> bool {
        match self {
            Self::Restore { page, depth } => {
                state.back_depth == *depth
                    && state.page == page.kind
                    && state.results_query == page.results_query
                    && (state.scroll_y - page.scroll_y).abs() <= 4.0
                    && state.video.as_ref().map(|video| &video.id) == page.video_id.as_ref()
            }
            Self::Watch { prior_id } => {
                state.page == "watch"
                    && state.video.as_ref().is_some_and(|video| {
                        !video.id.is_empty()
                            && video.first_frame
                            && prior_id.as_ref() != Some(&video.id)
                    })
            }
            Self::NoMenu => state.menu.is_none(),
            Self::Search(query) => {
                state.page == "search" && state.results_query.as_ref() == Some(query)
            }
            Self::Loaded => !state.loading,
        }
    }
}

#[derive(Debug)]
struct Pending {
    deadline: u64,
    expected: Expectation,
}

#[derive(Default)]
pub struct Properties {
    history: Vec<Page>,
    latest: Option<State>,
    pending: Vec<Pending>,
    settle_until: u64,
}

impl Properties {
    pub fn action(&mut self, elapsed_ms: u64, action: &Action) -> Result<(), Violation> {
        let state = self
            .latest
            .as_ref()
            .ok_or_else(|| violation(Property::Probe, "action before probe"))?;
        let requirement = match action {
            Action::Key { key } if key == "alt+Left" && state.back_depth > 0 => {
                let page = self.history.last().ok_or_else(|| {
                    violation(
                        Property::BackRestoration,
                        "back_depth has no corresponding observed page",
                    )
                })?;
                Some((
                    2_000,
                    Expectation::Restore {
                        page: page.clone(),
                        depth: state.back_depth - 1,
                    },
                ))
            }
            Action::Click {
                target,
                right: false,
            } if matches!(target.kind.as_str(), "card" | "row" | "related") => Some((
                6_000,
                Expectation::Watch {
                    prior_id: state.video.as_ref().map(|video| video.id.clone()),
                },
            )),
            Action::Key { key } if key == "Escape" && state.menu.is_some() => {
                Some((500, Expectation::NoMenu))
            }
            Action::Search { text, .. } => Some((6_000, Expectation::Search(text.clone()))),
            _ => None,
        };
        if let Some((within, expected)) = requirement {
            self.pending.push(Pending {
                deadline: elapsed_ms + within,
                expected,
            });
        }
        self.pending.push(Pending {
            deadline: elapsed_ms + 10_000,
            expected: Expectation::Loaded,
        });
        self.settle_until = elapsed_ms + 250;
        Ok(())
    }

    pub fn observe(&mut self, elapsed_ms: u64, state: State) -> Result<(), Violation> {
        self.check_deadlines(elapsed_ms)?;
        let valid_scroll =
            state.max_scroll.is_finite() && (0.0..=state.max_scroll).contains(&state.scroll_y);
        if !valid_scroll {
            return Err(violation(
                Property::ScrollBounds,
                format!("{} outside [0, {}]", state.scroll_y, state.max_scroll),
            ));
        }
        let expected_rail = match state.page.as_str() {
            "home" | "subscriptions" | "history" | "watch_later" => Some(state.page.as_str()),
            "search" | "watch" | "channel" => None,
            page => {
                return Err(violation(
                    Property::Probe,
                    format!("unexpected page {page:?}"),
                ));
            }
        };
        // The rail is not drawn in a narrow window, on Watch or in fullscreen, and a truthful
        // probe then reports no selection. Where it is drawn (rail targets on screen), a root
        // page's own entry is the selected one; other pages never select an entry.
        let rail_drawn = state.targets.iter().any(|target| target.kind == "rail");
        let rail_ok = match expected_rail {
            Some(_) if !rail_drawn => {
                state.rail_selected.is_none() || state.rail_selected.as_deref() == expected_rail
            }
            expected => state.rail_selected.as_deref() == expected,
        };
        if !rail_ok {
            return Err(violation(
                Property::RailSelection,
                format!(
                    "page {} has rail_selected {:?} (rail {})",
                    state.page,
                    state.rail_selected,
                    if rail_drawn { "drawn" } else { "not drawn" }
                ),
            ));
        }
        if let Some(previous) = &self.latest {
            if state.back_depth == self.history.len() + 1 {
                self.history.push(Page::from(previous));
            } else if state.back_depth <= self.history.len() {
                self.history.truncate(state.back_depth);
            } else {
                return Err(violation(
                    Property::Probe,
                    format!(
                        "unobserved history push: {} -> {}",
                        self.history.len(),
                        state.back_depth
                    ),
                ));
            }
        } else if state.back_depth != 0 {
            return Err(violation(Property::Probe, "initial history must be empty"));
        }
        self.pending.retain(|pending| {
            // A heartbeat can arrive before an input is delivered. Wait for the short
            // settling window before accepting an idle state as the response.
            (matches!(pending.expected, Expectation::Loaded) && elapsed_ms < self.settle_until)
                || !pending.expected.matches(&state)
        });
        self.latest = Some(state);
        Ok(())
    }

    pub fn check_deadlines(&self, elapsed_ms: u64) -> Result<(), Violation> {
        if let Some(expired) = self
            .pending
            .iter()
            .filter(|pending| elapsed_ms > pending.deadline)
            .min_by_key(|pending| pending.deadline)
        {
            return Err(violation(
                expired.expected.property(),
                format!(
                    "{:?} unresolved at {} ms (deadline {} ms)",
                    expired.expected, elapsed_ms, expired.deadline
                ),
            ));
        }
        Ok(())
    }

    pub fn settled(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn latest(&self) -> Option<&State> {
        self.latest.as_ref()
    }
}

pub fn health(alive: bool, pss_kib: Option<u64>) -> Result<(), Violation> {
    if !alive {
        return Err(violation(
            Property::Process,
            "app exited during exploration",
        ));
    }
    if let Some(pss_kib) = pss_kib
        && pss_kib.saturating_mul(1024) >= 1_500_000_000
    {
        return Err(violation(
            Property::Pss,
            format!("PSS {pss_kib} KiB (limit 1500 MB)"),
        ));
    }
    Ok(())
}

pub fn violation(property: Property, detail: impl Into<String>) -> Violation {
    Violation {
        property,
        detail: detail.into(),
        screenshot: None,
    }
}

#[cfg(test)]
mod tests;
