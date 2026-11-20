//! Commerce assistant — port of v1 `src/ai/agents/commerce.ts`
//! (Milestone 8, available since commerce is live).
//!
//! Reads the store's own data only, through the `wordless-commerce`
//! service functions: products, orders by status, revenue (paid +
//! fulfilled orders, in minor units), and low-stock warnings. Every
//! number is computed from `wl_products` / `wl_orders` — nothing is
//! invented, and the report says so. Drafts and proposals are not
//! part of this agent: it reports, it does not change the store.

use async_trait::async_trait;
use serde_json::json;
use wordless_kernel::error::WordlessError;

use crate::agents::{Agent, AgentContext};
use crate::types::{AgentResult, ProposalRisk};

/// Stock at or below this level counts as low stock (v1
/// `LOW_STOCK_AT_OR_BELOW`).
const LOW_STOCK_AT_OR_BELOW: i64 = 5;

pub struct CommerceAssistant;

#[async_trait(?Send)]
impl Agent for CommerceAssistant {
    fn id(&self) -> &'static str {
        "commerce-assistant"
    }
    fn name(&self) -> &'static str {
        "Commerce Assistant"
    }
    fn description(&self) -> &'static str {
        "Reports on the store from its own data: products, orders by status, revenue from paid orders, and low-stock warnings. It reports only — it never changes products or orders."
    }
    fn capabilities(&self) -> &[&'static str] {
        &[
            "Product and order reports",
            "Revenue totals (paid orders only)",
            "Low-stock warnings",
        ]
    }
    fn risk(&self) -> ProposalRisk {
        ProposalRisk::Low
    }
    fn requires_provider(&self) -> bool {
        false
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError> {
        let db = ctx.db;
        let products = wordless_commerce::products::list_products(db, None, 1, 100).await?;
        let orders = wordless_commerce::orders::list_orders(db, None, 1, 100).await?;
        let mut by_status: [(&str, i64); 4] = [
            ("pending", 0),
            ("paid", 0),
            ("fulfilled", 0),
            ("cancelled", 0),
        ];
        let mut revenue_minor = 0i64;
        let mut revenue_currency = "NGN".to_string();
        for order in &orders.orders {
            if let Some(entry) = by_status.iter_mut().find(|(s, _)| *s == order.status) {
                entry.1 += 1;
            }
            if order.status == "paid" || order.status == "fulfilled" {
                revenue_minor += order.total_minor;
                revenue_currency = order.currency.clone();
            }
        }
        let published = products
            .products
            .iter()
            .filter(|p| p.status == "published")
            .count() as i64;
        let low_stock: Vec<serde_json::Value> = products
            .products
            .iter()
            .filter(|p| (0..=LOW_STOCK_AT_OR_BELOW).contains(&p.stock))
            .map(|p| {
                json!({
                    "id": p.id,
                    "name": p.name,
                    "slug": p.slug,
                    "stock": p.stock,
                })
            })
            .collect();
        let orders_by_status = json!({
            "pending": by_status[0].1,
            "paid": by_status[1].1,
            "fulfilled": by_status[2].1,
            "cancelled": by_status[3].1,
        });
        let lines = [
            format!(
                "{} product(s) ({published} published).",
                products.products.len()
            ),
            format!(
                "{} order(s): {} pending, {} paid, {} fulfilled, {} cancelled.",
                orders.orders.len(),
                by_status[0].1,
                by_status[1].1,
                by_status[2].1,
                by_status[3].1
            ),
            format!(
                "Revenue from paid + fulfilled orders: {}.",
                wordless_commerce::money::format_money(revenue_minor, &revenue_currency)
            ),
            if low_stock.is_empty() {
                "No products are low on stock.".to_string()
            } else {
                format!(
                    "Low stock (≤ {LOW_STOCK_AT_OR_BELOW}): {}.",
                    low_stock
                        .iter()
                        .map(|p| format!(
                            "{} ({})",
                            p.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                            p.get("stock").and_then(|v| v.as_i64()).unwrap_or(0)
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
        ];
        Ok(AgentResult {
            summary: lines.join(" "),
            output: None,
            data: Some(json!({
                // v1 counts the sampled page (perPage 100), not the
                // table total — keep the same meaning here.
                "products": products.products.len(),
                "published": published,
                "orders": orders.orders.len(),
                "ordersByStatus": orders_by_status,
                "revenueMinor": revenue_minor,
                "revenueCurrency": revenue_currency,
                "revenueFormatted": wordless_commerce::money::format_money(revenue_minor, &revenue_currency),
                "lowStock": low_stock,
            })),
            proposals: None,
            usage: None,
        })
    }
}
