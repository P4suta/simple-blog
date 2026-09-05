# Vision

What simple-blog is for, who it serves, and what it will never be. Decisions
live in the [architecture decision records](adr/README.md); this document is
the reason behind them.

simple-blog exists so that one person can write on the web, at their own
address, for as long as they like, and never once have to think about the
software. The writer's whole job is the text. The reader's whole experience is
the text. Everything else, from hosting and publishing to backing up, keeping
the door locked, and moving house, is the software's job: done well enough to
be invisible, and honestly enough to be inspected whenever someone wants to
look.

## Who it serves

Four people. Every sentence below is written from one of their chairs.

**The writer** is one person with something to say. They write in Markdown
because it is the closest thing to plain text that still has headings. They
want to be read. They do not want to administer a server, learn a version
control system, remember a password, or fear that one wrong click will erase a
year of work.

**The reader** arrives from a link, a search, or a feed. They want the text,
now, on whatever device and connection they happen to have, in the language
the site is written in, dated in the place the site lives, and set in type
they can adjust to their own eyes. They are a guest. They are never a product.

**The operator** is usually the writer, running one binary on a machine at
home, and sometimes the official host running thousands of sites. Either way
they want to see that everything is healthy, and when something is not, to be
told exactly what and why, without a secret leaking into the explanation.

**The future self** is the writer years from now, moving to another host,
another domain, or away from simple-blog altogether. They lose nothing: not a
page, not a URL, not the passkey that opens the door.

## The ideal, in scenes

### Writing

The first minute. The writer runs one command, or reserves a domain with the
official host. They register a passkey. They write. There is no password to
invent, no email to confirm, no account to open with a provider, and no
configuration file to read first. The site learns its time zone from the
writer's browser and its language from one choice in the settings.

Every day after that. The writer opens the editor and writes. The shortcuts
they know from every other editor work here. Images go wherever they are
dropped and come out on the page at the right size, with no jump. Saving is
quiet and constant; if the tab dies, the words are still there when it comes
back. The preview is the real page in the real theme, not an imitation. A
second pair of eyes is one link away, and the link expires on its own.
Publishing is a button, or a minute on the clock in the site's own time, and
the piece appears at exactly that minute.

Mistakes. Every save is a revision the writer can return to. Deleting moves a
piece to the trash, where it waits, unchanged, until it is restored or the
writer empties the trash on purpose. Renaming a published piece leaves a
redirect behind, so no link anywhere in the world breaks. The writer never
hesitates over a button, because no button can cost them anything.

The site itself. A title, a description, an author, a navigation, a logo, a
favicon, and a stylesheet. Every setting is one a writer understands, is shown
with a live preview, and can be undone.

### Reading

A page is the text. It is an immutable file, so it arrives instantly from
anywhere in the world without asking a database or a template engine first.
Everything that matters works without JavaScript; where a script runs, it only
adds: highlighted search matches, a copy button on code, a like.

The type is right for the language: fonts that know CJK, ruby that renders,
lines that break where the language breaks them. Dates read the way the site's
readers expect, in the site's own hour. A long piece has a table of contents.
Code is highlighted when it is published, so it looks right without a script
and prints right on paper. Images carry their dimensions and load as the
reader scrolls, so nothing on the page shifts under their eyes.

Measure, text size, and colour scheme belong to the reader, not to the site.
They are remembered in the reader's own browser and applied before the first
paint.

Everything can be found: instant search, tags, an archive by year, related
pieces, the previous and the next, feeds in the formats readers actually use,
a sitemap, and canonical addresses that do not change.

Nothing is watched. No request leaves the page for a third party, no cookie is
set for a reader, and no counter is published. A like is a private note to the
writer, not a public score.

Every reader is welcome: a skip link, visible focus, respect for reduced
motion, a print stylesheet, meaningful markup, and structured data for the
machines that index the page.

### Owning

The site is a domain and a passkey. It is not an account with a provider, and
the provider does not hand out subdomains of its own as a substitute.

Markdown is the original. An export returns it as folders of plain files and
images that any other tool can read. A backup is one archive that holds
everything. A migration archive holds the entire site, with its history,
redirects, settings, media, trash, and passkeys, and moving it between
conforming hosts keeps every public address and the passkey identity intact.

Nothing is rationed. There is no post limit, no byte limit, and no traffic
tier invented by the product. The only limits are the ones that keep the
system safe, and each of them can be seen and explained.

Leaving is an ordinary operation, not a support ticket.

### Running

One binary, one database file, one data directory. There is no runtime Node,
no Git, no build pipeline to keep alive, and no scheduler to configure;
backups write themselves and rotate themselves.

A failure never takes the site down. The last complete release stays visible
while publication retries in the background; the dashboard says so and offers
to publish now. A half-published site cannot exist.

When something goes wrong, the software explains itself: a request ID on every
trace, a stable code for every failure, and a doctor that checks everything it
can reach and reports only what is safe to report.

The official host runs the same Core on the same contracts. Once a release is
active, a reader's request never touches the Core at all.

## Promises

These are the promises simple-blog makes and does not break. A change that
would break one of them changes this document first, in the open, or does not
happen.

1. **Nothing written is lost.** Revisions, a trash that restores exactly,
   drafts kept in the browser, backups on a schedule, atomic writes, and media
   cleanup that respects history.
2. **Nothing on the site lies.** Dates are the site's dates, a scheduled piece
   appears at its minute, a preview is the real theme, the search index is
   never stale, and an incomplete release is never visible.
3. **The writer's job is the text.** No configuration, no Git, no build step,
   no password. Addresses derive from titles, images simply work, and the
   editor stays out of the way.
4. **The reader is a guest, not a product.** No tracking, no accounts, no
   public metrics. Preferences belong to the reader, and the page works
   without a script.
5. **The site outlives the software.** Markdown is canonical, the whole site
   fits in one portable archive, the domain is the identity, and no provider
   subdomain stands in for it.
6. **Nothing is rationed.** No product quota on posts, bytes, or traffic.
   Safety limits exist, and every one of them is diagnosable.
7. **Every failure can be explained.** Diagnostics come first, error codes
   are stable, the doctor is thorough, and a secret never appears in a trace.
8. **It is quiet.** No notifications, no growth prompts, no streaks, no
   dashboard of vanity. The dashboard shows the writing.

## What it is not

- **Not a team CMS.** One owner, one site. No roles, no workflows, no
  approvals. Two people who write together run two sites.
- **Not a social network.** No comments, no followers, no public counts, no
  ranked feed. Conversation happens elsewhere; the site is the text.
- **Not a newsletter service, an analytics product, or an advertising
  surface.** Each of those makes the reader the product.
- **Not a page builder or a plugin platform.** There is one theme, and the
  writer can restyle it with a stylesheet. There is no marketplace.
- **Not a static-site generator.** The writer never sees Git or a build. The
  static release exists, but producing it is the software's job.
- **Not a framework.** It is a finished thing for one purpose.

## How the decisions serve the promises

| Promise | Decisions |
| --- | --- |
| Nothing written is lost | [0004](adr/0004-content-addressed-local-media.md), [0010](adr/0010-immutable-public-releases.md), [0011](adr/0011-portable-core-and-host-adapters.md), [0014](adr/0014-recoverable-trash.md) |
| Nothing on the site lies | [0010](adr/0010-immutable-public-releases.md), [0015](adr/0015-site-local-time.md), [0016](adr/0016-capability-preview-links.md) |
| The writer's job is the text | [0002](adr/0002-canonical-markdown-and-request-time-ssr.md), [0003](adr/0003-passkey-only-transaction-boundary.md), [0010](adr/0010-immutable-public-releases.md) |
| The reader is a guest, not a product | [0006](adr/0006-diagnostic-first-observability.md), [0013](adr/0013-dependency-free-static-highlighting.md), [0016](adr/0016-capability-preview-links.md) |
| The site outlives the software | [0002](adr/0002-canonical-markdown-and-request-time-ssr.md), [0011](adr/0011-portable-core-and-host-adapters.md), [0012](adr/0012-domain-first-official-hosting.md) |
| Nothing is rationed | [0012](adr/0012-domain-first-official-hosting.md) |
| Every failure can be explained | [0005](adr/0005-test-driven-delivery.md), [0006](adr/0006-diagnostic-first-observability.md), [0007](adr/0007-reproducible-verification-contract.md), [0008](adr/0008-no-unscoped-lint-suppression.md), [0009](adr/0009-protected-integration-boundary.md) |
| It is quiet | No record. Not every promise needs one; this one is kept by leaving things out. |

Decisions live only in the records. When a record and this document disagree
about behaviour, the record is right about behaviour, and this document is
corrected in the same change.

## The horizon

Not a roadmap, and not dated. These are the parts of the ideal that are still
owed. Each becomes a decision record and a failing test when work on it
begins.

- **Official hosting is live.** Reserve a domain, register a passkey, write:
  minutes after deciding to. The host adapter's contracts exist; the
  multi-site Core service behind them is not yet deployed.
- **Changing a domain loses nothing.** Public addresses and the passkey
  identity survive a move to a new name. The migration record already names
  this as a separate protocol.
- **Apex domains on the official host**, not only CNAME routing.
- **Writing from a phone is as good as writing from a desk.** A piece can be
  started, illustrated, previewed, and published from a pocket without
  compromise.
- **A second conforming host adapter.** Portability is proven by two
  implementations passing the same fixtures, not promised by one.

### Open questions

The vision does not answer these yet. Any answer must keep every promise
above.

- **Following.** Feeds are how a reader follows a site today. If readers need
  to hear about a new piece another way, by mail or through the fediverse, it
  has to arrive without a third-party tracker and without the writer running
  a mail server.
- **Media beyond images.** A writer may want to attach a document or a
  recording. What content-addressed, verified, and safe to serve means for
  those is undecided.
- **Writing back.** A reader may want to reply to the writer. Whether there is
  a form of that which never becomes moderation work is undecided; until it
  is, the answer is no.

## Using this document

This document says why. It does not say how, and it does not list features;
the README does that. Keep it short: no numbers, no dates, no version, no
setting names. A pull request that breaks a promise changes this document and
says so in its description. A pull request that adds a promise says who it
serves.
