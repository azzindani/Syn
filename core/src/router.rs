//! Model router: task class -> (model, effort). Cost-aware by construction:
//! Astra pixels are only routable to vision-fallback and deep-reasoning tasks.
//! Everything else resolves to Luna/Terra/Sol. Mirrors the OpenRouter picker
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
    Luna,
    Terra,
    Sol,
    Astra,
}

/// Reasoning effort ladder. Astra never routes below Low or above High
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
        TaskKind::Skim => Route { model: Model::Luna, effort: Effort::Low, cost_rank: 1 },
        TaskKind::Routine => Route { model: Model::Terra, effort: Effort::Medium, cost_rank: 2 },
        TaskKind::Code => Route { model: Model::Sol, effort: Effort::Medium, cost_rank: 3 },
        TaskKind::DeepReasoning => Route { model: Model::Astra, effort: Effort::High, cost_rank: 4 },
        TaskKind::VisionFallback => Route { model: Model::Astra, effort: Effort::Low, cost_rank: 4 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn astra_only_for_hard_tasks() {
        for t in [TaskKind::Skim, TaskKind::Routine, TaskKind::Code] {
            assert_ne!(route(t).model, Model::Astra, "{t:?} must not route to Astra");
        }
        assert_eq!(route(TaskKind::VisionFallback).model, Model::Astra);
        assert_eq!(route(TaskKind::DeepReasoning).model, Model::Astra);
    }

    #[test]
    fn cost_ranks_order() {
        assert!(route(TaskKind::Skim).cost_rank < route(TaskKind::Routine).cost_rank);
        assert!(route(TaskKind::Routine).cost_rank < route(TaskKind::Code).cost_rank);
    }
}
