## MODIFIED Requirements

### Requirement: Release Packaging

QuickFox SHALL provide a tag-triggered GitHub Release workflow for 1.0.0 and later releases.

#### Scenario: Tag creates macOS and Windows release assets

- **WHEN** tag `v1.0.0` is pushed
- **THEN** GitHub Actions SHALL build macOS and Windows Tauri bundles
- **AND** upload the generated installer artifacts to a GitHub Release

#### Scenario: Ordinary CI remains verification-only

- **WHEN** a branch push or pull request runs ordinary CI
- **THEN** CI SHALL run checks and tests
- **AND** SHALL NOT publish release assets
