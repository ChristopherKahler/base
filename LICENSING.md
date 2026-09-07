# Licensing

The short version: **you can use base at work.** You can run it commercially,
inside a company of any size, modify it, and build products with it. The one
thing you cannot do is sell base itself, or sell a service that stands in for
base or for basemode.

Two years after any given version is published, that version becomes Apache
2.0. That grant is irrevocable and it is written into the license, not into a
promise on a web page.

## What covers what

| Surface | License |
| --- | --- |
| The engine — everything in `src/`, the `base` binary, `Cargo.toml`, `tests/` | [FSL-1.1-ALv2](LICENSE.md), converting to Apache-2.0 two years after each version ships |
| Extensions, adapters and skills published by this project | [Apache-2.0](LICENSE-APACHE-2.0) |
| Extensions and adapters written by anyone else | Yours. You own them. See the exception below |
| Your graph, your notes, your code, anything base reads or writes on your machine | Yours. No license is asserted over your data, and none is needed |

## Plain-English guide to the engine license

Permitted, with no separate agreement and no conversation with us:

- Running base inside a company, including a company that sells software.
- Building, shipping and selling a product that uses base internally.
- Modifying the source, running your fork internally, and redistributing your
  changes under these same terms.
- Consulting and professional services delivered to someone who is themselves
  using base under this license.
- Teaching with it, and research with it.

Not permitted without a separate commercial license:

- Offering base, or a rebadged base, as a product or a hosted service.
- Offering a commercial service that substitutes for basemode.
- Shipping something that provides the same or substantially similar
  functionality as base as your commercial product.

If you want to do one of those, that is a conversation and not a refusal.
Reach out through [basemode.ai](https://basemode.ai).

## Extension interface exception

This is an additional permission granted by the licensor. It only adds rights.

An **Extension** is any work that interacts with base solely through its
documented public interfaces: the extension manifest and extension API, command
plugins, the hook protocol, the `base` command-line surface, and the on-disk
graph format.

For the avoidance of doubt:

1. An Extension is an independent work and is **not** a derivative work of the
   Software, regardless of how it is loaded, invoked or distributed.
2. The author of an Extension owns it outright and may license it under any
   terms they choose, including proprietary terms.
3. Writing, distributing or selling an Extension is **not** a Competing Use,
   including where the Extension is sold commercially.
4. This exception does not extend to an Extension whose own purpose is to
   substitute for the Software or for basemode.

The point of this clause is that the adapter and platform-map ecosystem should
belong to the people who build it. Nothing you write against base's interfaces
becomes ours, and nothing you write against them is encumbered by our license.

## Versions published before this change

Every release up to and including **v0.14.1** was published under the PolyForm
Noncommercial License 1.0.0, retained verbatim at
[LICENSE-PolyForm-Noncommercial-1.0.0.md](LICENSE-PolyForm-Noncommercial-1.0.0.md).
That grant is not withdrawn and cannot be. If you received one of those
versions, you keep the rights you were given for that version, permanently.

Releases after v0.14.1 carry the terms in [LICENSE.md](LICENSE.md), which grant
strictly more than the terms they replace.

If you held off on using base because the old license did not permit commercial
use: it does now, and it does retroactively in the only sense that matters,
which is that the current version is the one you would install.

## Trademarks

The license grants no trademark rights. "basemode" and the basemode marks
identify software published by Chris Kahler. A fork is welcome and permitted;
calling a fork basemode is not.

## Contributing

Contributions are accepted under the inbound terms in
[CONTRIBUTING.md](CONTRIBUTING.md). Read that section before opening a pull
request; it is short and it exists so that a future license correction never
requires chasing down contributors.

## Not legal advice

This page is a plain-English guide written by the maintainer to help you decide
quickly. Where it and [LICENSE.md](LICENSE.md) disagree, LICENSE.md governs.
