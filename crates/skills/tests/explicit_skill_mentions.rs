use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use devo_skills::{
    SkillLoadOutcome, SkillMetadata, SkillScope, SkillSelection, collect_explicit_skill_mentions,
};
use pretty_assertions::assert_eq;

fn skill(name: &str, path: &str) -> SkillMetadata {
    SkillMetadata {
        name: name.to_string(),
        description: format!("{name} description"),
        short_description: None,
        interface: None,
        dependencies: None,
        policy: None,
        path_to_skills_md: PathBuf::from(path),
        scope: SkillScope::Repo,
        plugin_id: None,
    }
}

fn outcome(skills: Vec<SkillMetadata>) -> SkillLoadOutcome {
    SkillLoadOutcome {
        skills,
        ..SkillLoadOutcome::default()
    }
}

#[test]
fn explicit_skill_mentions_preserve_selection_order_and_skip_duplicates() {
    let alpha = skill("alpha", "/repo/alpha/SKILL.md");
    let beta = skill("beta", "/repo/beta/SKILL.md");
    let gamma = skill("gamma", "/repo/gamma/SKILL.md");
    let duplicate_a = skill("duplicate", "/repo/duplicate-a/SKILL.md");
    let duplicate_b = skill("duplicate", "/repo/duplicate-b/SKILL.md");

    let mut load_outcome = outcome(vec![
        alpha.clone(),
        beta.clone(),
        gamma.clone(),
        duplicate_a,
        duplicate_b,
    ]);
    load_outcome
        .disabled_paths
        .insert(PathBuf::from("/repo/beta/SKILL.md"));

    let actual = collect_explicit_skill_mentions(
        &[
            "Use [$gamma](/repo/gamma/SKILL.md), $alpha, $duplicate, and $PATH".to_string(),
            "Mentioning $alpha again should not duplicate it".to_string(),
        ],
        &[SkillSelection {
            name: "alpha".to_string(),
            path: PathBuf::from("/repo/alpha/SKILL.md"),
        }],
        &load_outcome,
    );

    assert_eq!(actual, vec![alpha, gamma]);
}

#[test]
fn deep_research_skill_is_selected_from_explicit_dollar_mention() {
    let deep_research = skill("deep-research", "/system/skills/deep-research/SKILL.md");
    let load_outcome = outcome(vec![deep_research.clone()]);

    let actual = collect_explicit_skill_mentions(
        &["$deep-research investigate indexing strategies".to_string()],
        &[],
        &load_outcome,
    );

    assert_eq!(actual, vec![deep_research]);
}

#[test]
#[ignore]
fn bench_collect_explicit_skill_mentions_without_mentions() {
    let skills = (0..2_000)
        .map(|index| {
            skill(
                &format!("skill-{index}"),
                &format!("/repo/skills/skill-{index}/SKILL.md"),
            )
        })
        .collect::<Vec<_>>();
    let load_outcome = outcome(skills);
    let texts = (0..128)
        .map(|index| format!("regular prompt line {index} with no explicit skill reference"))
        .collect::<Vec<_>>();

    let started = Instant::now();
    let mut total_selected = 0;
    for _ in 0..200 {
        total_selected += black_box(collect_explicit_skill_mentions(
            black_box(&texts),
            black_box(&[]),
            black_box(&load_outcome),
        ))
        .len();
    }
    let elapsed = started.elapsed();

    assert_eq!(total_selected, 0);
    println!(
        "collect_explicit_skill_mentions_without_mentions iterations=200 skills=2000 texts=128 elapsed_ms={} per_call_us={:.2}",
        elapsed.as_secs_f64() * 1_000.0,
        elapsed.as_secs_f64() * 1_000_000.0 / 200.0
    );
}

#[test]
#[ignore]
fn bench_collect_explicit_skill_mentions_with_sparse_mentions() {
    let skills = (0..2_000)
        .map(|index| {
            skill(
                &format!("skill-{index}"),
                &format!("/repo/skills/skill-{index}/SKILL.md"),
            )
        })
        .collect::<Vec<_>>();
    let load_outcome = outcome(skills);
    let mut texts = (0..126)
        .map(|index| format!("regular prompt line {index} with no explicit skill reference"))
        .collect::<Vec<_>>();
    texts.push("Use $skill-7 for this turn".to_string());
    texts.push("Also use [$skill-19](/repo/skills/skill-19/SKILL.md)".to_string());

    let started = Instant::now();
    let mut total_selected = 0;
    for _ in 0..200 {
        total_selected += black_box(collect_explicit_skill_mentions(
            black_box(&texts),
            black_box(&[]),
            black_box(&load_outcome),
        ))
        .len();
    }
    let elapsed = started.elapsed();

    assert_eq!(total_selected, 400);
    println!(
        "collect_explicit_skill_mentions_with_sparse_mentions iterations=200 skills=2000 texts=128 elapsed_ms={} per_call_us={:.2}",
        elapsed.as_secs_f64() * 1_000.0,
        elapsed.as_secs_f64() * 1_000_000.0 / 200.0
    );
}
