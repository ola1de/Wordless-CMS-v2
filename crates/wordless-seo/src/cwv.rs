//! Core Web Vitals budgets (P10, doc §0.1 F).
//!
//! Budgets are the web.dev "good" thresholds (LCP < 2.5 s, INP <
//! 200 ms, CLS < 0.1), stored as the `wl_cwv_budgets` wp_options
//! JSON value so an admin can tighten or loosen them. They are
//! budgets, not measurements: the Worker never measures a visitor's
//! vitals itself (no RUM beacon is shipped — that would be invented
//! analytics), and no page is ever claimed to pass them. An audit
//! run ([`crate::audit`]) records measured values from a real tool
//! (Lighthouse on staging); [`evaluate_cwv`] compares a run's
//! numbers against these budgets.

use serde::{Deserialize, Serialize};
use wordless_db::Db;
use wordless_kernel::error::{bad_request, WordlessError};

pub const CWV_OPTION: &str = "wl_cwv_budgets";

/// LCP/INP in milliseconds; CLS as a unitless ratio.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CwvBudgets {
    pub lcp_ms: u64,
    pub inp_ms: u64,
    pub cls: f64,
}

impl Default for CwvBudgets {
    /// web.dev "good" thresholds.
    fn default() -> Self {
        Self {
            lcp_ms: 2500,
            inp_ms: 200,
            cls: 0.1,
        }
    }
}

/// Per-metric pass/fail against the budgets, plus an overall verdict.
/// A `None` measurement is "not measured" (never counted as pass).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CwvEvaluation {
    pub lcp_pass: Option<bool>,
    pub inp_pass: Option<bool>,
    pub cls_pass: Option<bool>,
    /// True only when every measured metric passes AND at least one
    /// metric was measured.
    pub pass: bool,
}

pub fn evaluate_cwv(
    budgets: &CwvBudgets,
    lcp_ms: Option<u64>,
    inp_ms: Option<u64>,
    cls: Option<f64>,
) -> CwvEvaluation {
    let lcp_pass = lcp_ms.map(|v| v <= budgets.lcp_ms);
    let inp_pass = inp_ms.map(|v| v <= budgets.inp_ms);
    let cls_pass = cls.map(|v| v <= budgets.cls);
    let any_measured = lcp_pass.is_some() || inp_pass.is_some() || cls_pass.is_some();
    let pass = any_measured
        && lcp_pass.unwrap_or(true)
        && inp_pass.unwrap_or(true)
        && cls_pass.unwrap_or(true);
    CwvEvaluation {
        lcp_pass,
        inp_pass,
        cls_pass,
        pass,
    }
}

/// P12: CWV budgets are a per-site option.
pub async fn get_cwv_budgets(db: &dyn Db, site_id: i64) -> Result<CwvBudgets, WordlessError> {
    let stored: Option<CwvBudgets> = wordless_multisite::get_site_option(db, site_id, CWV_OPTION, None)
        .await
        .map_err(WordlessError::from)?;
    Ok(stored.unwrap_or_default())
}

/// Validate + store budgets. Ranges keep a typo from making the
/// budget meaningless: LCP 500–10,000 ms, INP 50–1,000 ms,
/// CLS 0.01–1.0.
pub async fn set_cwv_budgets(
    db: &dyn Db,
    site_id: i64,
    budgets: CwvBudgets,
) -> Result<CwvBudgets, WordlessError> {
    if !(500..=10_000).contains(&budgets.lcp_ms) {
        return Err(bad_request(
            "Field \"lcpMs\" must be between 500 and 10000.",
        ));
    }
    if !(50..=1_000).contains(&budgets.inp_ms) {
        return Err(bad_request("Field \"inpMs\" must be between 50 and 1000."));
    }
    if !(0.01..=1.0).contains(&budgets.cls) || !budgets.cls.is_finite() {
        return Err(bad_request("Field \"cls\" must be between 0.01 and 1."));
    }
    wordless_multisite::set_site_option(db, site_id, CWV_OPTION, &budgets)
        .await
        .map_err(WordlessError::from)?;
    Ok(budgets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;

    #[test]
    fn defaults_are_the_web_dev_good_thresholds() {
        let b = CwvBudgets::default();
        assert_eq!(b.lcp_ms, 2500);
        assert_eq!(b.inp_ms, 200);
        assert!((b.cls - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn evaluation_passes_only_when_every_measured_metric_passes() {
        let b = CwvBudgets::default();
        let all_good = evaluate_cwv(&b, Some(2000), Some(150), Some(0.05));
        assert!(all_good.pass);
        let one_bad = evaluate_cwv(&b, Some(3000), Some(150), Some(0.05));
        assert!(!one_bad.pass);
        assert_eq!(one_bad.lcp_pass, Some(false));
        let unmeasured = evaluate_cwv(&b, None, None, None);
        assert!(!unmeasured.pass);
        assert_eq!(unmeasured.lcp_pass, None);
        let partial = evaluate_cwv(&b, Some(2000), None, None);
        assert!(partial.pass);
    }

    #[test]
    fn budgets_round_trip_and_reject_out_of_range_values() {
        let db = SqliteDb::new();
        let saved = block_on(set_cwv_budgets(
            &db, 1,
            CwvBudgets {
                lcp_ms: 2000,
                inp_ms: 100,
                cls: 0.05,
            },
        ))
        .unwrap();
        assert_eq!(saved.lcp_ms, 2000);
        assert_eq!(block_on(get_cwv_budgets(&db, 1)).unwrap().inp_ms, 100);
        assert!(block_on(set_cwv_budgets(
            &db, 1,
            CwvBudgets {
                lcp_ms: 100,
                ..Default::default()
            }
        ))
        .is_err());
        assert!(block_on(set_cwv_budgets(
            &db, 1,
            CwvBudgets {
                cls: 5.0,
                ..Default::default()
            }
        ))
        .is_err());
    }
}
