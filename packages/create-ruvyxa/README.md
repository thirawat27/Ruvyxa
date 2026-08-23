# create-ruvyxa

Create a Ruvyxa app from a minimal or focused file-system route starter.

## Usage

```bash
npm create ruvyxa@latest my-app
cd my-app
npm install
npm run dev
```

Choose a starter with `--template` (or `-t` when invoking the binary directly):

```bash
npm create ruvyxa@latest my-blog -- --template blog
npm create ruvyxa@latest my-admin -- --template crud
npm create ruvyxa@latest my-api -- --template api
```

Run it with no arguments on a terminal and it becomes interactive: it prompts for a project name and
lets you pick the starter with an arrow-key menu (`j`/`k` also navigate). The scaffold output shows
the real generated file tree — nested, directories first, colored by role — and runs under an
animated mascot spinner.

Available starters are `minimal` (the default), `blog`, `crud`, and `api`.

The generated project starts with:

```text
AGENTS.md
CLAUDE.md
.gitignore
app/globals.css
app/layout.tsx
app/page.tsx
public/ruvyxa.png
package.json
ruvyxa.config.ts
tsconfig.json
```

The minimal starter stays intentionally small. Focused starters add a blog with static parameters, a
CRUD flow with an API and validated action, or an API-only backend while keeping the same public
framework conventions. Read the repository [User Guide](../../docs/README.md) for the end-to-end app
workflow and [Development & Testing](../../docs/en/12-development-testing.md) for contributor
checks.

## Project Names

Project names must be valid directory names for the target operating system. On Windows, reserved
device names such as `con`, `prn`, and `aux` are rejected, and names cannot end with unsafe trailing
characters.
