# Skill Resources Design

## Goal

Make Neo skills first-class local packages without turning the skill system into a hosted marketplace or a heavy plugin runtime.

Neo already treats a skill as a directory with a `SKILL.md`, and the loaded skill keeps its root path. That is enough to support Codex-style progressive disclosure, but the contract is implicit. This design makes the contract explicit and extends `CreateSkill` so skill-authoring workflows can create the supporting files they refer to.

The target shape is:

```text
skill-name/
  SKILL.md
  references/   # optional read-on-demand documentation
  scripts/      # optional deterministic helpers
  assets/       # optional templates or output assets
```

`SKILL.md` remains the only file Neo loads automatically. Resources are local files under the skill root. The skill body must explicitly route future agents to those files through `${NEO_SKILL_DIR}`.

This design also updates the built-in `create-skill` and `self-evo` prompts because both author skills:

- `create-skill` creates a skill from a user requirement.
- `self-evo` distills session history into reusable skills.

They should share the same resource contract while keeping different product intent.

## Current State

Neo's current skill model is intentionally small:

- `LoadedSkill` stores `name`, `root`, `manifest`, `body`, and `source`.
- `load_skill_file` reads only `SKILL.md`, parses frontmatter, and stores the body.
- `expand_skill_body` already replaces `${NEO_SKILL_DIR}` with the skill root.
- `CreateSkill` currently accepts only `name`, `description`, `skill_type`, and `body`.
- `CreateSkill` writes only `~/.neo/skills/<name>/SKILL.md`.
- `MoveSkill` moves a skill directory and backs up existing destinations.
- Discovery recursively finds any directory containing `SKILL.md`; when a parent skill contains child skills, child names become `parent/child`.

That means resource-backed skills are possible by hand today, but not productized. A user can manually add `references/guide.md` and write instructions that point at `${NEO_SKILL_DIR}/references/guide.md`. Neo will not stop this. But `CreateSkill` cannot create those files, docs do not define the contract, and discovery may accidentally treat a `SKILL.md` inside a resource directory as a nested skill.

## Design Choice

Use the light local-first package contract and extend `CreateSkill` with optional resources.

Rejected alternatives:

- **Prompt-only contract:** Keep `CreateSkill` as `SKILL.md` only and tell agents to ask for follow-up resource creation. This preserves minimal code but leaves an awkward gap where the recommended skill design cannot be created by the built-in authoring skill.
- **Full Codex-style package system:** Add initializer scripts, UI metadata, resource generators, and rich package validation. This is too large for the first Neo iteration and conflicts with the current local-only, typed-Rust tool surface.

The chosen design gives Neo enough structure to author useful resource-backed skills while keeping the runtime simple: no automatic resource loading, no remote registry, no binary asset pipeline, and no new plugin semantics.

## Product Principles

`SKILL.md` is the entry point. It is the only resource automatically injected into model context.

Resources are progressive disclosure. They exist to avoid stuffing long references, scripts, schemas, examples, and templates into `SKILL.md`.

Resources are local files. Neo does not fetch, publish, sync, or index them remotely.

Resource use is explicit. Skill bodies must say when to read a reference, when to run a script, and when to copy or adapt an asset.

The contract must be cross-platform. Paths are relative, normalized, and validated with `Path`/`PathBuf` semantics. No shell-specific assumptions.

Resource directories are not nested-skill directories. Nested skills should live under `skills/`; `references/`, `scripts/`, and `assets/` are reserved non-discovery zones.

## Non-Goals

This design does not add a marketplace, package registry, hosted sync, or team distribution service.

This design does not add UI metadata such as Codex's `agents/openai.yaml`.

This design does not add a general-purpose file-write API. Resource writes are scoped to a skill package created by `CreateSkill`.

This design does not support binary resource payloads in `CreateSkill` v1. Text assets and templates are supported through UTF-8 strings. Binary assets remain manual follow-up work.

This design does not automatically read `references/` into context, run `scripts/`, or inspect `assets/`.

This design does not change skill activation semantics, auto-invocation, or permission modes.

## Package Layout Contract

The canonical skill package layout is:

```text
<skill-root>/
  SKILL.md
  references/
    schema.md
  scripts/
    validate_schema.py
  assets/
    template.md
```

Only `SKILL.md` is required. All resource directories are optional.

### `references/`

Use for documentation that should be loaded only when needed:

- API docs
- schemas
- command references
- domain policies
- long examples
- troubleshooting tables

Reference files should be text. Long reference files should include their own table of contents or grep-friendly headings. `SKILL.md` should say exactly when to read each reference.

Example:

```markdown
For OAuth setup details, read `${NEO_SKILL_DIR}/references/oauth.md` before editing config.
```

### `scripts/`

Use for deterministic helpers that agents would otherwise rewrite repeatedly or get wrong:

- parsers
- validators
- converters
- generators
- focused test helpers

Scripts are local files; Neo does not automatically execute them. `SKILL.md` must give the command and verification expectation. Scripts created by `CreateSkill` may request an executable bit on Unix, but the skill must still work cross-platform by invoking the interpreter explicitly when needed.

Example:

```markdown
Run `python ${NEO_SKILL_DIR}/scripts/validate_schema.py <file>` to validate generated output.
```

### `assets/`

Use for files used in final outputs:

- text templates
- boilerplate snippets
- sample config files
- prompt templates
- static text fixtures

In v1, `CreateSkill` supports text assets only. Binary files can still be placed manually in `assets/`, but the built-in authoring flow must report that binary assets need follow-up work.

### Reserved Resource Directories

The directory names `references`, `scripts`, and `assets` are reserved at the top level of a skill package. They are not scanned for nested skills.

Nested skills should use the existing bundle shape:

```text
parent-skill/
  SKILL.md
  skills/
    child-skill/
      SKILL.md
```

This prevents accidental skill registration when references include examples named `SKILL.md`.

## `CreateSkill` Tool Contract

Extend `CreateSkillArgs` with an optional `resources` array:

```rust
pub struct CreateSkillArgs {
    pub name: String,
    pub description: String,
    pub skill_type: String,
    pub body: String,
    #[serde(default)]
    pub resources: Vec<CreateSkillResource>,
}

pub struct CreateSkillResource {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub executable: bool,
}
```

JSON example:

```jsonc
{
  "name": "pdf-rotate",
  "description": "Use when rotating PDF pages or normalizing PDF page orientation.",
  "skill_type": "prompt",
  "body": "# PDF Rotate\n\n## Workflow\nRead `${NEO_SKILL_DIR}/references/pdf.md` for PDF constraints. Run `python ${NEO_SKILL_DIR}/scripts/rotate_pdf.py input.pdf output.pdf --pages 1-3 --degrees 90`.\n\n## Verify\nOpen or render the output PDF and confirm the requested pages changed orientation.",
  "resources": [
    {
      "path": "references/pdf.md",
      "content": "# PDF Notes\n\nUse pypdf for page rotation. Verify output by rendering the changed pages."
    },
    {
      "path": "scripts/rotate_pdf.py",
      "content": "import argparse\n\nparser = argparse.ArgumentParser()\nparser.add_argument('input')\nparser.add_argument('output')\nargs = parser.parse_args()\nprint(f'rotate {args.input} -> {args.output}')\n",
      "executable": true
    },
    {
      "path": "assets/rotation-request-template.md",
      "content": "Pages: 1-3\nDegrees: 90\n"
    }
  ]
}
```

### Resource Path Validation

Each resource path must pass all checks before any file is written:

- path is relative;
- path is not empty;
- path uses `/` in tool input, then Neo converts through `PathBuf`;
- first component is exactly `references`, `scripts`, or `assets`;
- path has at least one component after the resource directory;
- no component is empty, `.`, or `..`;
- no component is a Windows reserved device basename;
- no component ends with `.` on Windows-hostile paths;
- no path attempts to escape the skill root after normalization;
- resource path cannot be `SKILL.md`;
- resource path cannot target a directory.

Reject invalid resources with a `CreateSkill` invalid-input error that names the offending path and rule.

### Content Limits

`CreateSkill` v1 accepts UTF-8 text content only.

To keep tool calls and transcripts sane:

- each resource content string should be capped at 256 KiB;
- total resource content in a single call should be capped at 1 MiB;
- empty content is allowed only for `assets/` templates when the skill body explains how the asset is filled.

If the user needs larger references or binary assets, `create-skill` should ask for a follow-up manual or tool-assisted resource step instead of shoving a large blob through `CreateSkill`.

### Write Semantics

`CreateSkill` writes the full package under `~/.neo/skills/<name>/`.

The operation should be all-or-reject for validation: validate `name`, frontmatter serialization, every resource path, and content limits before creating or overwriting any files.

When creating a new skill:

1. Create the skill directory if needed.
2. Write `SKILL.md` atomically.
3. Create resource parent directories.
4. Write each resource file atomically.
5. Apply executable mode where supported and requested.
6. Reload the shared skill store.

When overwriting an existing skill:

1. Validate all inputs first.
2. Back up the entire existing skill directory to `~/.neo/backups/skills/<timestamp>/<name>/`.
3. Replace `SKILL.md` and the provided resource files.
4. Do not delete existing resource files that are not mentioned in the request.
5. Reload the shared skill store.

Not deleting unmentioned resources avoids accidental data loss when updating only the skill body. If a future cleanup command is needed, it should be explicit rather than hidden inside `CreateSkill`.

### Symlink and Reparse Safety

Existing safety checks for `SKILL.md` must extend to resource writes:

- reject writes through existing symlinks;
- reject writes through reparse points on Windows;
- reject resource parent directories that are symlinks or reparse points;
- ensure backup copying does not follow unsafe links out of the skill root.

The implementation should reuse the existing safe-directory, child-directory, atomic-write, and backup helpers where possible rather than creating a separate path-safety policy.

### Executable Flag

`executable: true` requests executable permissions for scripts. Behavior:

- on Unix, set owner-executable plus existing readable bits after writing;
- on Windows, do not attempt POSIX mode changes;
- never rely on executable bits for cross-platform instructions;
- generated skills should prefer explicit interpreter commands such as `python ${NEO_SKILL_DIR}/scripts/tool.py`.

The `executable` flag is allowed for any resource path but only meaningful for `scripts/`. `create-skill` should set it only for scripts.

## `ListSkills` Contract

`ListSkills` should remain concise. Add a resource summary suffix when a skill has top-level resource directories:

```text
[user]
  pdf-rotate: /Users/me/.neo/skills/pdf-rotate [references,scripts]
```

It should not list individual files by default. The goal is discoverability, not a file tree dump.

If a resource directory exists but is empty, omit it from the summary. If reading a resource directory fails, omit the summary rather than failing the entire skill list; skill discovery should continue to focus on loadable skills.

## Discovery Contract

`discover_skills` should skip the top-level resource directories of a skill package:

- `references/`
- `scripts/`
- `assets/`

This skip applies when the directory is inside a package root that has `SKILL.md`.

Examples:

```text
foo/
  SKILL.md
  references/SKILL.md
```

Discovers only `foo`.

```text
foo/
  SKILL.md
  skills/bar/SKILL.md
```

Discovers `foo` and `foo/bar`.

```text
skills-root/
  references/SKILL.md
```

If `skills-root` itself is just a configured skill root and not a skill package, this remains discoverable as `references` unless the directory is under a package root. This preserves existing root-level directory behavior while reserving resource dirs within packages.

## Built-in Skill Updates

### `create-skill`

`create-skill` should be updated from "design resources but ask for a follow-up" to "design resources and use `CreateSkill.resources` when needed."

Required guidance:

- collect concrete usage examples before drafting;
- classify the skill as workflow, technique, reference, tool integration, or discipline-enforcing;
- decide whether resource files improve the skill;
- if using resources, create them in `references/`, `scripts/`, or `assets/`;
- ensure `SKILL.md` explicitly routes when to read or run each resource;
- do not create placeholder resources;
- do not put large references in `SKILL.md` when `references/` is more appropriate;
- do not use resources for one-off session logs or narrative history;
- include `## Verify` that checks behavior, not just file existence;
- after `CreateSkill`, call `ListSkills` and report the path, backup status, reload status, and resources created.

`create-skill` should remain manually invoked through `disableModelInvocation: true`.

### `self-evo`

`self-evo` should be updated because it also creates skills, but its input is session history rather than a fresh user requirement.

Required guidance:

- identify repeatable workflows, techniques, references, scripts, and assets from the selected scope;
- do not create skills for one-off project context, transient logs, or facts that belong in project docs or memory;
- when session history contains a reusable command, script, schema, checklist, or template, consider resource files instead of bloating `SKILL.md`;
- create one focused skill at a time unless the scope clearly contains independent repeatable workflows;
- include `## Verify` in every generated skill;
- call `CreateSkill` with `resources` when resources are useful;
- call `ListSkills` and verify every created skill is visible;
- report created skill names, resources, backup paths, and reload status.

`self-evo` should also preserve its existing no-argument behavior: no argument is not a scope; ask for a concrete scope before summarizing.

## Documentation Updates

Update both English and Chinese skill docs:

- define the local package layout;
- explain `references/`, `scripts/`, and `assets/`;
- state that `SKILL.md` is the only automatic context entry point;
- document `${NEO_SKILL_DIR}`;
- show `CreateSkill.resources` examples;
- explain that `references/`, `scripts/`, and `assets/` are not scanned for nested skills under a package root;
- document that nested skills belong under `skills/`;
- update built-in `create-skill` and `self-evo` descriptions if needed.

Docs should avoid promising binary asset support through `CreateSkill` v1.

## Testing Strategy

Use narrow tests; do not run broad workspace test suites as evidence.

### Tool Tests

Add or update `neo-agent-core` tests for `CreateSkill`:

- creates `SKILL.md` plus `references/foo.md`;
- creates `scripts/tool.py` and preserves content;
- creates `assets/template.md`;
- rejects absolute resource paths;
- rejects `..` escapes;
- rejects resource paths outside `references/`, `scripts/`, or `assets/`;
- rejects direct `SKILL.md` resource path;
- rejects symlink/reparse resource targets;
- backs up the full existing skill directory before overwrite;
- preserves unmentioned existing resources when updating a package;
- reloads the skill store after resource-backed creation.

Add or update `ListSkills` tests:

- shows `[references]`, `[scripts]`, or `[assets]` style summary for skills with resources;
- omits empty resource dirs;
- keeps output concise.

Add discovery tests:

- `references/SKILL.md` under a package is not discovered;
- `scripts/SKILL.md` under a package is not discovered;
- `assets/SKILL.md` under a package is not discovered;
- `skills/child/SKILL.md` under a package is still discovered as `parent/child`.

### Built-in Skill Tests

Update built-in tests so:

- `create-skill` body mentions `CreateSkill.resources`;
- `create-skill` body mentions `${NEO_SKILL_DIR}`;
- `self-evo` body mentions resource-backed skill creation;
- both built-ins still require `## Verify`;
- both built-ins remain manual-only where intended.

### Verification Commands

Expected focused verification after implementation:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::<exact_create_skill_resource_test> --exact --nocapture
cargo test --package neo-agent-core --lib -- skills::discovery::tests::<exact_resource_discovery_test> --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::<exact_builtin_prompt_test> --exact --nocapture
cargo fmt --all --check
git diff --check -- <touched-files>
```

Use the exact test names created during implementation rather than broad filters.

## Error Handling

All `CreateSkill` validation failures should be `ToolError::InvalidInput` with actionable messages:

- invalid resource path;
- unsupported resource directory;
- resource content too large;
- total resource content too large;
- resource target is a symlink or unsafe reparse point;
- resource target collides with a directory.

I/O failures should remain `ToolError::Io`.

Reload failure should preserve the current behavior: report that the file package was written but the active session could not reload it. The tool should not roll back a successful file write only because reload failed.

## Migration and Compatibility

Existing skills continue to work unchanged.

Existing `CreateSkill` calls without `resources` continue to work because `resources` defaults to an empty array.

Existing hand-authored resource directories continue to work. The new contract documents them and prevents accidental nested-skill discovery under package resource dirs.

No compatibility alias is needed for alternate directory names such as `reference/`, `docs/`, or `examples/`. The canonical names are `references/`, `scripts/`, and `assets/`.

The current uncommitted prompt-only improvement to `create-skill` should be updated in the implementation rather than preserved as a parallel old path.

## Security and Trust

Resources are local files and may be read or executed only through normal Neo tool permissions and workspace/trust policy.

Creating scripts does not grant execution. Future execution still goes through existing command/tool approval semantics.

Resource path validation must not rely on string prefix checks alone. Use normalized path components and existing safe path helpers.

Do not auto-run newly created scripts as part of `CreateSkill`. Verification belongs to the agent workflow after creation and must use normal tools.

## Open Decisions Resolved

`CreateSkill` should support optional resources in v1.

Resources are text-only through `CreateSkill` v1.

`SKILL.md` remains the only automatic context entry point.

Top-level `references/`, `scripts/`, and `assets/` inside a skill package are reserved non-discovery directories.

Nested skills should use `skills/`.

Unmentioned existing resources are preserved on overwrite.

## Self-Review

Placeholder scan: no unresolved placeholder markers remain.

Consistency check: the package layout, `CreateSkill.resources`, discovery skip rules, and built-in skill updates all use the same canonical directory names.

Scope check: the work is suitable for one implementation plan. It touches `CreateSkill`, `ListSkills`, discovery, built-in skill prompts, docs, and focused tests, but does not require a new runtime subsystem.

Ambiguity check: binary resource support, nested skill location, resource deletion, and automatic resource loading are explicitly out of scope or resolved.
