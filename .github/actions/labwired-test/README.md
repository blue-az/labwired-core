# Core labwired-test action

This directory contains the Core action used by release smoke tests. For a
public workflow, use the root LabWired action below: it downloads the pinned
Core release archive rather than compiling Rust on the consumer's runner.

~~~yaml
- name: Run LabWired tests
  uses: w1ne/labwired-core/.github/actions/labwired-test@main
  with:
    version: v0.18.0
    script: tests/firmware-test.yaml
    output-dir: out/labwired
    args: --no-uart-stdout
~~~

The action source is referenced at `main` because no post-hardening Core action
tag exists yet. The `version` input independently selects the immutable Core CLI
release archive named
`labwired-v0.18.0-<platform>.tar.gz`.

The local Core action still uses its hyphenated names for its internal release
smoke workflow. Its inputs are `script` (required), `version`, `args`, `junit`,
`output-dir`, `upload-artifacts`, `repo`, and `github-token`.
