# Packages and Marketplace

Neo packages are tar archives described by a sibling `.neo-package.toml`
manifest. The same format is used for extension packages, prompt packs, and
theme packages.

## Manifest Format

```toml
kind = "extension" # "extension", "prompt-pack", or "theme"
id = "echo"
version = "0.1.0"
entry = "neo-extension.toml"

[archive]
path = "echo-0.1.0.tar"
sha256 = "<lowercase-or-uppercase-hex-sha256-of-archive>"

[signature]
algorithm = "ed25519"
public_key = "<base64-raw-32-byte-ed25519-public-key>"
signature = "<base64-raw-64-byte-ed25519-signature-over-archive-bytes>"
```

The `entry` field is the package's primary file:

- `extension`: usually `neo-extension.toml`.
- `prompt-pack`: a packaged `.md` prompt template.
- `theme`: a packaged `.json` theme file.

Package ids must be safe single path segments using ASCII letters, digits,
`.`, `_`, or `-`. Archive references, entry paths, and archive member paths
must be relative paths without `..`.

## Validation

Before install or publish, Neo validates the package manifest and archive:

- Reads `.neo-package.toml` and verifies `kind`, `id`, `version`, `entry`,
  archive digest, and signature metadata.
- Verifies the archive bytes against `archive.sha256`.
- Verifies the Ed25519 package signature over the exact archive bytes.
- Rejects absolute archive paths, `..` components, unsafe ids, and missing
  package entry files.
- Rejects archive member paths that are absolute, contain `..`, or are empty.
- Rejects symlinks and hard links whose targets escape the package root.

Current trust policy: manifest self-sign only. The public key lives in the
downloaded manifest, and Neo verifies that key against the archive bytes. Neo
does not yet bind that key to a publisher identity, marketplace account, local
trust store, transparency log, or root trust anchor. Treat this as archive
integrity plus tamper detection for the manifest/archive pair, not a complete
publisher/root trust chain.

Validated packages install under the project-local roots:

- Extensions: `.neo/extensions/<id>`
- Prompt packs: `.neo/prompts/<id>`
- Themes: `.neo/themes/<id>`

Prompt and theme discovery recursively scans those package install roots, while
explicit prompt-template directory selectors remain non-recursive.

## CLI

Marketplace operations require `NEO_MARKETPLACE_URL`. Neo fails closed when the
server is unavailable or the environment variable is not set.

```bash
export NEO_MARKETPLACE_URL=http://localhost:8080

cargo run -p neo-agent -- extensions search echo
cargo run -p neo-agent -- extensions install echo@0.1.0 --from marketplace
cargo run -p neo-agent -- extensions publish path/to/.neo-package.toml

cargo run -p neo-agent -- prompts search review
cargo run -p neo-agent -- prompts install review-pack@1.0.0 --from marketplace
cargo run -p neo-agent -- prompts publish path/to/.neo-package.toml
cargo run -p neo-agent -- prompts list
cargo run -p neo-agent -- prompts preview review

cargo run -p neo-agent -- themes search night
cargo run -p neo-agent -- themes install night-owl@2.0.0 --from marketplace
cargo run -p neo-agent -- themes publish path/to/.neo-package.toml
cargo run -p neo-agent -- themes list
cargo run -p neo-agent -- themes preview night-owl
```

## Marketplace HTTP DTOs

Search:

```http
GET /api/v1/marketplace/packages/search?kind=extension&q=echo
```

```json
{
  "packages": [
    {
      "kind": "extension",
      "id": "echo",
      "version": "0.1.0",
      "name": "Echo",
      "description": "Echo extension",
      "publisher": "neo-test"
    }
  ]
}
```

Resolve:

```http
GET /api/v1/marketplace/packages/extension/echo/0.1.0
```

```json
{
  "package": {
    "kind": "extension",
    "id": "echo",
    "version": "0.1.0",
    "manifest_url": "/api/v1/marketplace/packages/extension/echo/0.1.0/.neo-package.toml",
    "archive_url": "/api/v1/marketplace/packages/extension/echo/0.1.0/echo-0.1.0.tar"
  }
}
```

Publish:

```http
POST /api/v1/marketplace/packages/publish
```

```json
{
  "manifest": {
    "kind": "extension",
    "id": "echo",
    "version": "0.1.0",
    "entry": "neo-extension.toml",
    "archive": {
      "path": "echo-0.1.0.tar",
      "sha256": "<hex>"
    },
    "signature": {
      "algorithm": "ed25519",
      "public_key": "<base64>",
      "signature": "<base64>"
    }
  },
  "archive_base64": "<base64-tar-bytes>"
}
```

The publish response returns the same package record shape used by search.
