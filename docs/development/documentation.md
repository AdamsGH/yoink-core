# Documentation and Wiki

Repository Markdown is the source of truth. The Wiki export uses the public pages listed in [navigation.json](../navigation.json).

## Check documents

```sh
just test-docs
just check-docs
```

The checker validates local Markdown links and headings, public-page navigation coverage, unique Wiki names, and exclusion of agent instructions from Wiki pages.

It does not prove that prose matches implementation or that an external service is reachable. Review commands, defaults, routes, and authorization against their owning source.

## Export the Wiki

```sh
just wiki
```

The default destination is `backend/target/wiki`. To select another directory:

```sh
just wiki /absolute/path/to/wiki-export
```

The exporter rewrites public document links to Wiki pages. Source-code links point to the core repository. It generates `Home.md`, `_Sidebar.md`, and topic pages.

`docs/agents/` never enters the export. Agent instructions remain available only in the repository.

The output is generated. Do not edit it as a second documentation source. Regenerate it after changing canonical pages.

## Publish the export

GitHub must initialize the Wiki before its Git repository exists. Create the first page through the repository's Wiki tab if necessary.

Clone `https://github.com/AdamsGH/yoink-core.wiki.git` into a separate checkout. Review the generated files before copying them into that checkout.

Preserve unrelated Wiki pages. Remove retired generated pages only after comparing the previous export manifest. Do not delete unknown files or replace remote history.

Commit the reviewed Wiki files with the configured human identity. Push only when publication is authorized.

An export is not a published Wiki. Report the remote verification result separately.
