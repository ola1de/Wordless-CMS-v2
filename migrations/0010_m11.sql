-- Milestone 11 — plugin compatibility reports.
--
-- Additive only: one nullable JSON column on wl_extensions holding the
-- per-plugin compatibility report produced by the M4 PHP analyser at
-- install time (see src/modules/integrations/compat-report.ts). NULL
-- means "not analysed" (extensions installed before M11, or installs
-- with no PHP files). Existing rows are untouched.
ALTER TABLE wl_extensions ADD COLUMN report_json TEXT;
