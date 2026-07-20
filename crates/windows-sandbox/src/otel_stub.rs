//! No-op telemetry stubs until Devo wires OTEL for Windows sandbox setup.
//!
//! These types mirror the Devo setup-helper surface so `cfg(windows)` modules
//! (e.g. `wfp_setup`) compile against a stable API without pulling core OTEL.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsigMetricsSettings {
    pub environment: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OtelExporter {
    #[default]
    None,
    Statsig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OtelSettings {
    pub environment: String,
    pub service_name: String,
    pub service_version: String,
    pub devo_home: PathBuf,
    pub exporter: OtelExporter,
    pub trace_exporter: OtelExporter,
    pub metrics_exporter: OtelExporter,
    pub runtime_metrics: bool,
    pub span_attributes: BTreeMap<String, String>,
    pub tracestate: BTreeMap<String, String>,
}

#[derive(Debug, Default)]
pub struct OtelMetrics;

impl OtelMetrics {
    pub fn counter(&self, _name: &str, _inc: u64, _tags: &[(&str, &str)]) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct OtelProvider;

impl OtelProvider {
    pub fn from(_settings: &OtelSettings) -> anyhow::Result<Self> {
        Ok(Self)
    }

    pub fn metrics(&self) -> Option<OtelMetrics> {
        None
    }

    pub fn shutdown(&self) {}
}

pub fn global_statsig_metrics_settings() -> Option<StatsigMetricsSettings> {
    None
}
