# Image publication

Local deployment and remote publication are separate operations. A successful local rollout does not establish that GitHub built or published the release.

## Published images

Release images target Linux amd64:

| Image | Contents |
| --- | --- |
| `backend` | Native bot/API and confined workers. |
| `frontend` | Built Mini App. |
| `nginx` | HTTP edge configuration. |
| `youtube-services` | YouTube providers and Unix gateway. |
| `importer` | Offline PostgreSQL importer. |
| `operations` | Native backup/restore utility. |

The registry prefix is `ghcr.io/adamsgh/yoink-core`. Release image tags match `backend/VERSION`.

The local TDLight Bot API compatibility build is not in this matrix. Provision it separately.

## Publish a release

1. Verify the source and current documentation.
2. Commit changed submodules before updating their core references.
3. Update `backend/VERSION` and the runtime pin in `.env.example`.
4. Run the applicable [verification gates](../development/verification.md).
5. Push the release commit and matching `vMAJOR.MINOR.PATCH` tag.
6. Wait for the tag workflow's `required` job to succeed.
7. Create the GitHub Release on that tag with concise release notes.

The workflow scans the histories of core and the four submodules. It runs source checks, builds the frontend, and builds the six images.

Master builds use commit-addressed tags. SemVer tags must match the bundle version. Existing published image tags cannot move to another source commit.

A failed required job means publication is incomplete, even if individual images already exist. Diagnose the failure before retrying it.

The workflow and [image matrix](../../scripts/container-images.mjs) define the publication contract. Publication never deploys a service or changes production data.

## Registry access

GitHub can create private GHCR packages for a public repository. Package owners must configure visibility before claiming that anonymous pulls work.

Use registry authentication until anonymous access is verified. Do not place tokens in documentation, command arguments, or committed configuration.

## Documentation and Wiki

Repository documentation is canonical. Generate the Wiki export from the same checkout after documentation checks pass.

The [documentation workflow](../development/documentation.md) describes the export and first-page requirement. Agent instructions are not part of the Wiki.
