//! Model router: task class -> (model, effort). Cost-aware by construction:
//! Reasoning pixels are only routable to vision-fallback and deep-reasoning tasks.
//! Everything else resolves to Small/Standard/Coding. Mirrors the OpenRouter picker
//! in the widget: user choice overrides, router is the default.

/// Task classes the planner emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    Skim,
    Routine,
    Code,
    DeepReasoning,
    VisionFallback,
}

/// Routable models (OpenRouter IDs resolved widget-side).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    Small,
    Standard,
    Coding,
    Reasoning,
}

impl Model {
    /// Every routable slot, cheapest first.
    ///
    /// The one list. Anything that needs to walk the slots -- the config
    /// banner, the failover chain, the model picker -- reads it from here,
    /// so adding a slot is one edit in the module that owns the type rather
    /// than a literal array copied into three files that drift apart.
    pub const ALL: [Model; 4] = [Model::Small, Model::Standard, Model::Coding, Model::Reasoning];
}

/// The job class a slot is the default for: the inverse of `route`.
pub fn task_of(model: Model) -> TaskKind {
    match model {
        Model::Small => TaskKind::Skim,
        Model::Standard => TaskKind::Routine,
        Model::Coding => TaskKind::Code,
        Model::Reasoning => TaskKind::DeepReasoning,
    }
}

/// The route a slot runs under when picked directly.
pub fn route_of(model: Model) -> Route {
    route(task_of(model))
}

/// The CLI/UI name for a job class, and back again. One table, so the
/// command surface and the picker cannot disagree about what `code` means.
pub fn task_name(task: TaskKind) -> &'static str {
    match task {
        TaskKind::Skim => "skim",
        TaskKind::Routine => "routine",
        TaskKind::Code => "code",
        TaskKind::DeepReasoning => "deep",
        TaskKind::VisionFallback => "vision",
    }
}

pub fn task_named(name: &str) -> Option<TaskKind> {
    Some(match name {
        "skim" => TaskKind::Skim,
        "routine" => TaskKind::Routine,
        "code" => TaskKind::Code,
        "deep" => TaskKind::DeepReasoning,
        "vision" => TaskKind::VisionFallback,
        _ => return None,
    })
}

/// Reasoning effort ladder. Reasoning never routes below Low or above High
/// without explicit user override (widget enforces).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Effort {
    Low,
    Medium,
    High,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub model: Model,
    pub effort: Effort,
    /// Relative cost rank 1 (cheapest) to 4. For budget display, not billing.
    pub cost_rank: u8,
}

pub fn route(task: TaskKind) -> Route {
    match task {
        TaskKind::Skim => Route { model: Model::Small, effort: Effort::Low, cost_rank: 1 },
        TaskKind::Routine => Route { model: Model::Standard, effort: Effort::Medium, cost_rank: 2 },
        TaskKind::Code => Route { model: Model::Coding, effort: Effort::Medium, cost_rank: 3 },
        TaskKind::DeepReasoning => Route { model: Model::Reasoning, effort: Effort::High, cost_rank: 4 },
        TaskKind::VisionFallback => Route { model: Model::Reasoning, effort: Effort::Low, cost_rank: 4 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn astra_only_for_hard_tasks() {
        for t in [TaskKind::Skim, TaskKind::Routine, TaskKind::Code] {
            assert_ne!(route(t).model, Model::Reasoning, "{t:?} must not route to Reasoning");
        }
        assert_eq!(route(TaskKind::VisionFallback).model, Model::Reasoning);
        assert_eq!(route(TaskKind::DeepReasoning).model, Model::Reasoning);
    }

    #[test]
    fn cost_ranks_order() {
        assert!(route(TaskKind::Skim).cost_rank < route(TaskKind::Routine).cost_rank);
        assert!(route(TaskKind::Routine).cost_rank < route(TaskKind::Code).cost_rank);
    }

    #[test]
    fn every_slot_is_listed_once_and_cheapest_first() {
        let ranks: Vec<u8> = Model::ALL.iter().map(|m| route_of(*m).cost_rank).collect();
        assert!(ranks.windows(2).all(|w| w[0] <= w[1]), "ALL must be cheapest first: {ranks:?}");
        let mut seen = Model::ALL.to_vec();
        seen.dedup();
        assert_eq!(seen.len(), Model::ALL.len(), "no slot may appear twice");
    }

    #[test]
    fn task_of_is_the_inverse_of_route() {
        // If these drift, `task code` and the picker's "code" stop meaning
        // the same model and nobody finds out until a run goes to the wrong
        // slot.
        for m in Model::ALL {
            assert_eq!(route(task_of(m)).model, m, "task_of({m:?}) does not route back");
        }
    }

    #[test]
    fn task_names_round_trip() {
        for t in [TaskKind::Skim, TaskKind::Routine, TaskKind::Code, TaskKind::DeepReasoning, TaskKind::VisionFallback] {
            assert_eq!(task_named(task_name(t)), Some(t));
        }
        assert_eq!(task_named("nonsense"), None);
    }
}
