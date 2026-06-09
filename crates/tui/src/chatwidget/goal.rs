//! Rendering and local UI actions for the `/goal` command.

use ratatui::style::Stylize;
use ratatui::text::Line;

use devo_protocol::ThreadGoal;
use devo_protocol::ThreadGoalStatus;

use crate::app_command::AppCommand;
use crate::app_command::GoalObjectiveMode;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::CustomPromptView;
use crate::bottom_pane::list_selection_view::ListSelectionView;
use crate::bottom_pane::list_selection_view::SelectionItem;
use crate::bottom_pane::list_selection_view::SelectionViewParams;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::history_cell;
use crate::history_cell::PlainHistoryCell;
use crate::status_indicator_widget::fmt_elapsed_compact;

use super::ChatWidget;

impl ChatWidget {
    pub(super) fn show_goal_status(&mut self, goal: Option<ThreadGoal>) {
        match goal {
            Some(goal) => {
                self.add_to_history(PlainHistoryCell::new(goal_summary_lines(&goal)));
                self.set_status_message("Goal shown");
            }
            None => {
                self.add_to_history(history_cell::new_info_event(
                    "Usage: /goal <objective>".to_string(),
                    Some("No goal is currently set.".to_string()),
                ));
                self.set_status_message("No goal set");
            }
        }
    }

    pub(super) fn show_goal_updated(&mut self, goal: ThreadGoal) {
        self.add_to_history(history_cell::new_info_event(
            format!("Goal {}", goal_status_label(goal.status)),
            Some(goal_usage_summary(&goal)),
        ));
        self.set_status_message("Goal updated");
    }

    pub(super) fn show_goal_replace_confirmation(
        &mut self,
        current_goal: ThreadGoal,
        objective: String,
    ) {
        let replace_objective = objective.clone();
        let items = vec![
            SelectionItem {
                name: "Replace current goal".to_string(),
                description: Some("Set the new objective and start it now".to_string()),
                actions: vec![Box::new(move |tx: &AppEventSender| {
                    tx.send(AppEvent::Command(AppCommand::set_goal_objective(
                        replace_objective.clone(),
                        GoalObjectiveMode::ReplaceExisting,
                    )));
                })],
                dismiss_on_select: true,
                ..SelectionItem::default()
            },
            SelectionItem {
                name: "Cancel".to_string(),
                description: Some("Keep the current goal".to_string()),
                actions: vec![Box::new(|tx: &AppEventSender| {
                    tx.send(AppEvent::StatusMessageChanged {
                        message: "Goal unchanged".to_string(),
                    });
                })],
                dismiss_on_select: true,
                ..SelectionItem::default()
            },
        ];
        self.bottom_pane
            .open_popup_view(Box::new(ListSelectionView::new(
                SelectionViewParams {
                    title: Some("Replace goal?".to_string()),
                    subtitle: Some(format!("Current: {}", current_goal.objective)),
                    footer_hint: Some(standard_popup_hint_line()),
                    items,
                    ..SelectionViewParams::default()
                },
                self.app_event_tx.clone(),
                self.active_accent_color(),
            )));
        self.add_to_history(history_cell::new_info_event(
            "Goal already exists".to_string(),
            Some(format!("New objective: {objective}")),
        ));
        self.set_status_message("Confirm goal replacement");
    }

    pub(super) fn show_goal_edit_prompt(&mut self, goal: ThreadGoal) {
        let tx = self.app_event_tx.clone();
        let current_objective = goal.objective.clone();
        let status = goal.status;
        let token_budget = goal.token_budget;
        let view = CustomPromptView::new(
            "Edit goal".to_string(),
            "Type a goal objective and press Enter".to_string(),
            None,
            Box::new(move |objective: String| {
                tx.send(AppEvent::Command(AppCommand::set_goal_objective(
                    objective,
                    GoalObjectiveMode::UpdateExisting {
                        status,
                        token_budget,
                    },
                )));
            }),
        )
        .with_initial_text(&current_objective);
        self.bottom_pane.open_popup_view(Box::new(view));
        self.set_status_message("Editing goal");
    }

    pub(super) fn show_goal_cleared(&mut self, cleared: bool) {
        if cleared {
            self.add_to_history(history_cell::new_info_event(
                "Goal cleared".to_string(),
                None,
            ));
            self.set_status_message("Goal cleared");
        } else {
            self.add_to_history(history_cell::new_info_event(
                "No goal to clear".to_string(),
                Some("This session does not currently have a goal.".to_string()),
            ));
            self.set_status_message("No goal set");
        }
    }

    pub(super) fn show_goal_operation_failed(&mut self, message: String) {
        self.add_to_history(history_cell::new_error_event_with_hint(
            message,
            Some("goal operation failed".to_string()),
        ));
        self.set_status_message("Goal operation failed");
    }
}

fn goal_summary_lines(goal: &ThreadGoal) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from("Goal".bold()),
        Line::from(vec![
            "Status: ".dim(),
            goal_status_label(goal.status).to_string().into(),
        ]),
        Line::from(vec!["Objective: ".dim(), goal.objective.clone().into()]),
        Line::from(vec![
            "Time used: ".dim(),
            fmt_elapsed_compact(goal.time_used_seconds.max(0) as u64).into(),
        ]),
        Line::from(vec![
            "Tokens used: ".dim(),
            format_tokens_compact(goal.tokens_used).into(),
        ]),
    ];
    if let Some(token_budget) = goal.token_budget {
        lines.push(Line::from(vec![
            "Token budget: ".dim(),
            format_tokens_compact(token_budget).into(),
        ]));
    }
    let command_hint = match goal.status {
        ThreadGoalStatus::Active => "Commands: /goal edit, /goal pause, /goal clear",
        ThreadGoalStatus::Paused => "Commands: /goal edit, /goal resume, /goal clear",
        ThreadGoalStatus::BudgetLimited | ThreadGoalStatus::Complete => {
            "Commands: /goal edit, /goal clear"
        }
    };
    lines.push(Line::default());
    lines.push(Line::from(command_hint.dim()));
    lines
}

fn goal_status_label(status: ThreadGoalStatus) -> &'static str {
    match status {
        ThreadGoalStatus::Active => "active",
        ThreadGoalStatus::Paused => "paused",
        ThreadGoalStatus::BudgetLimited => "limited by budget",
        ThreadGoalStatus::Complete => "complete",
    }
}

fn goal_usage_summary(goal: &ThreadGoal) -> String {
    let mut parts = vec![format!("Objective: {}", goal.objective)];
    if goal.time_used_seconds > 0 {
        parts.push(format!(
            "Time: {}.",
            fmt_elapsed_compact(goal.time_used_seconds as u64)
        ));
    }
    if let Some(token_budget) = goal.token_budget {
        parts.push(format!(
            "Tokens: {}/{}.",
            format_tokens_compact(goal.tokens_used),
            format_tokens_compact(token_budget)
        ));
    }
    parts.join(" ")
}

fn format_tokens_compact(tokens: i64) -> String {
    let tokens = tokens.max(0);
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}
