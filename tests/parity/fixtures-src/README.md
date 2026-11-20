# Test fixtures (source files)

`hello.php` is the Hello Dolly plugin (version 1.7.2) exactly as bundled
with WordPress 7.1.5. It is WordPress code, licensed GPL-2.0-or-later,
used here only as a test fixture: the parity harness builds
`hello-dolly.zip` from it to exercise the plugin marketplace install,
compatibility report, activate and deactivate flows.

The parity script prefers the copy from a local WordPress reference
checkout when one exists and falls back to this file, so the harness
runs on a machine that has never seen the reference tree.
