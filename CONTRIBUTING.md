# Contributing

Thank you for helping. Tacie is maintained by one person, so reviews may take a while.

## Before you start

For anything bigger than a small fix, open an issue first and say what you want to change. A change to how the app
looks is easier to agree on before the code is written.

## Building and testing

See the README for what to install. Then:

```sh
(cd engine && cargo test --workspace)    # engine
cd apps/mac && ./scripts/build.sh && ./scripts/test.sh    # app (build once before the tests)
```

Keep your change in the style of the code around it. The engine is Rust, the app Swift (SwiftUI and AppKit). Please
add tests for changes in behavior.

## Sign your commits (DCO)

Every commit needs a `Signed-off-by` line, which says you wrote the change or have the right to submit it under the MIT
License ([Developer Certificate of Origin](https://developercertificate.org)):

```sh
git commit -s
```

There is no contributor license agreement.

## How a pull request lands

Tacie is developed in a private repository, and this one is exported from it. An accepted pull request is applied
there with its author and sign-off kept, and comes back here with a later export. The pull request is then closed with
a note naming the commit it went into.

Contributed code can also ship in the official app, which updates itself. Changes to build scripts, `project.yml` (which pins the Swift
packages) or `Cargo.lock` are reviewed with extra care for that reason.
