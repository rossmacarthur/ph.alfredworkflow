# ph.alfredworkflow

[![Build Status](https://badgers.space/github/checks/rossmacarthur/ph.alfredworkflow?label=build)](https://github.com/rossmacarthur/ph.alfredworkflow/actions/workflows/build.yaml?query=branch%3Atrunk)
[![Latest Release](https://badgers.space/github/release/rossmacarthur/ph.alfredworkflow)](https://github.com/rossmacarthur/ph.alfredworkflow/releases/latest)

⚙️ Alfred workflow to search Phabricator/Phorge repos, diffs, tasks, and
documents.

## Features

- Pulls config from ~/.arcrc.
- Search repositories, and open the selected repository in your browser
- Search differential revisions, and open the selected diff in your browser
- Search maniphest tasks, and open the selected task in your browser
  - Full text search of task titles and content
- Search phriction documents, and open the selected document in your browser
  - Full text search of document titles and content
- Blazingly fast 🤸.

## 📦 Installation

### Pre-packaged

Grab the latest release from
[the releases page](https://github.com/rossmacarthur/ph.alfredworkflow/releases).

Because the release contains an executable binary later versions of macOS will
mark it as untrusted and Alfred won't be able to execute it. You can run the
following to explicitly trust the release before installing to Alfred.
```sh
xattr -c ~/Downloads/github-*-apple-darwin.alfredworkflow
```

### Building from source

This workflow is written in Rust, so to install it from source you will first
need to install Rust and Cargo using [rustup](https://rustup.rs/). Then install
[powerpack](https://github.com/rossmacarthur/powerpack). Then you can run the
following to build an `.alfredworkflow` file.

```sh
git clone https://github.com/rossmacarthur/ph.alfredworkflow.git
cd ph.alfredworkflow
powerpack package
```

The release will be available at `target/workflow/ph.alfredworkflow`.

## License

This project is distributed under the terms of both the MIT license and the
Apache License (Version 2.0).

See [LICENSE-APACHE](LICENSE-APACHE) and [LICENSE-MIT](LICENSE-MIT) for details.
