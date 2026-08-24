use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[cfg(test)]
use std::path::Path;

use anyhow::Context;

use crate::{
    AddArgs, AddTemplate, accent, dim, info, load_project_config, note, number, ok_text,
    print_field, print_header, print_success_banner,
};

struct ScaffoldFile {
    relative: &'static str,
    content: &'static str,
}

pub(crate) fn scaffold_add(args: AddArgs) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let config = load_project_config(&args.root)?;
    let app_dir = PathBuf::from(config.app_dir());
    let mut files = BTreeMap::<PathBuf, &'static str>::new();
    for template in args.templates {
        for file in template_files(template) {
            // Joined one component at a time. `join("a/b")` keeps the literal
            // slash inside the path, which on Windows printed a mixed
            // `app\form-example/action.ts` in the created-file list.
            let relative = file
                .relative
                .split('/')
                .fold(app_dir.clone(), |path, component| path.join(component));
            files.insert(relative, file.content);
        }
    }
    let conflicts = files
        .keys()
        .filter(|relative| args.root.join(relative).exists())
        .cloned()
        .collect::<Vec<_>>();
    if !args.force && !conflicts.is_empty() {
        anyhow::bail!(
            "RUV2401 scaffold would overwrite existing files:\n{}\nRun again with --force only if these files are scaffold-owned.",
            conflicts
                .iter()
                .map(|path| format!("  {}", path.display()))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    for (relative, content) in &files {
        let target = args.root.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(&target, content)
            .with_context(|| format!("failed to write {}", target.display()))?;
    }

    // This command used to print its own header and hand-count its own column
    // padding, which is how it ended up as the one command with no colour and a
    // field column two spaces off from every other.
    print_header("Adds");
    print_field("scaffolds", number(files.len().to_string()));
    println!();

    let created = files.keys().collect::<Vec<_>>();
    for (index, relative) in created.iter().enumerate() {
        let branch = if index + 1 == created.len() {
            "└─"
        } else {
            "├─"
        };
        println!(
            "  {} {} {}",
            dim(branch),
            ok_text("new"),
            accent(relative.display().to_string())
        );
    }

    if files.keys().any(|path| path.ends_with("_server/auth.ts")) {
        println!();
        print_field(
            "dependency",
            info(format!("@ruvyxa/auth ^{}", env!("CARGO_PKG_VERSION"))),
        );
        print_field(
            "next",
            note("install the dependency, then set RUVYXA_AUTH_SECRET"),
        );
    }

    print_success_banner(
        format!("Scaffolded {} file(s)", files.len()),
        started.elapsed(),
    );
    Ok(())
}

fn template_files(template: AddTemplate) -> &'static [ScaffoldFile] {
    match template {
        AddTemplate::Form => FORM_FILES,
        AddTemplate::DataTable => DATA_TABLE_FILES,
        AddTemplate::Auth => AUTH_FILES,
    }
}

const FORM_FILES: &[ScaffoldFile] = &[
    ScaffoldFile {
        relative: "form-example/action.ts",
        content: r#"import { action } from 'ruvyxa/server'

export const submitContact = action
  .input({
    parse(value: unknown) {
      if (!value || typeof value !== 'object') throw new Error('Form input is required')
      const input = value as Record<string, unknown>
      const email = String(input.email ?? '').trim().toLowerCase()
      const message = String(input.message ?? '').trim()
      if (!/^\S+@\S+\.\S+$/.test(email)) throw new Error('A valid email is required')
      if (message.length < 10 || message.length > 2000) {
        throw new Error('Message must contain 10-2000 characters')
      }
      return { email, message }
    },
  })
  .handler(async ({ input, invalidate }) => {
    invalidate('contacts')
    return { accepted: true, email: input.email }
  })
"#,
    },
    ScaffoldFile {
        relative: "form-example/page.tsx",
        content: r#"export default function FormExamplePage() {
  return (
    <main>
      <h1>Validated contact form</h1>
      <form method="post" action="/__ruvyxa/action?path=/form-example&name=submitContact">
        <label>Email <input name="email" type="email" required /></label>
        <label>Message <textarea name="message" minLength={10} maxLength={2000} required /></label>
        <button type="submit">Send</button>
      </form>
    </main>
  )
}
"#,
    },
];

const DATA_TABLE_FILES: &[ScaffoldFile] = &[ScaffoldFile {
    relative: "_components/ruvyxa/data-table.tsx",
    content: r#"'use client'

import { useMemo, useState } from 'react'

export interface DataColumn<TRow> {
  key: keyof TRow
  label: string
  render?: (value: TRow[keyof TRow], row: TRow) => React.ReactNode
}

export function DataTable<TRow extends Record<string, unknown>>({
  rows,
  columns,
  rowKey,
}: {
  rows: readonly TRow[]
  columns: readonly DataColumn<TRow>[]
  rowKey: keyof TRow
}) {
  const [query, setQuery] = useState('')
  const [sortKey, setSortKey] = useState<keyof TRow | null>(null)
  const visible = useMemo(() => {
    const filtered = rows.filter((row) =>
      Object.values(row).some((value) => String(value).toLowerCase().includes(query.toLowerCase())),
    )
    if (!sortKey) return filtered
    return [...filtered].sort((left, right) =>
      String(left[sortKey]).localeCompare(String(right[sortKey]), undefined, { numeric: true }),
    )
  }, [query, rows, sortKey])

  return (
    <section>
      <label>Filter <input value={query} onChange={(event) => setQuery(event.currentTarget.value)} /></label>
      <table>
        <thead><tr>{columns.map((column) => (
          <th key={String(column.key)}><button type="button" onClick={() => setSortKey(column.key)}>{column.label}</button></th>
        ))}</tr></thead>
        <tbody>{visible.map((row) => (
          <tr key={String(row[rowKey])}>{columns.map((column) => (
            <td key={String(column.key)}>{column.render?.(row[column.key], row) ?? String(row[column.key] ?? '')}</td>
          ))}</tr>
        ))}</tbody>
      </table>
    </section>
  )
}
"#,
}];

const AUTH_FILES: &[ScaffoldFile] = &[
    ScaffoldFile {
        relative: "_server/auth.ts",
        content: r#"import { createAuth, memoryAuthStore, memoryRateLimitStore } from '@ruvyxa/auth'

const development = process.env.NODE_ENV !== 'production'
const secret = process.env.RUVYXA_AUTH_SECRET ?? (development ? 'development-only-secret-change-me-now' : '')
if (secret.length < 32) throw new Error('RUVYXA_AUTH_SECRET must contain at least 32 characters')

export const auth = createAuth({
  secret,
  origin: process.env.RUVYXA_AUTH_ORIGIN ?? 'http://localhost:3000',
  // Memory stores are bounded and development-only. Replace both with durable,
  // atomic stores before a production build; @ruvyxa/auth fails closed otherwise.
  store: memoryAuthStore({ development: true }),
  rateLimitStore: memoryRateLimitStore({ development: true }),
  providers: {
    credentials: {
      type: 'credentials',
      async authorize(input) {
        const email = String(input.email ?? '').trim().toLowerCase()
        const password = String(input.password ?? '')
        if (email !== process.env.RUVYXA_DEMO_USER || password !== process.env.RUVYXA_DEMO_PASSWORD) return null
        return { id: email, email }
      },
    },
  },
})
"#,
    },
    ScaffoldFile {
        relative: "__ruvyxa/auth/[...path]/route.ts",
        content: r#"import { auth } from '../../../_server/auth.js'

async function handle({ request }: { request: Request }) {
  return (await auth.handle(request)) ?? new Response('Auth route not found', { status: 404 })
}

export { handle as GET, handle as POST }
"#,
    },
    ScaffoldFile {
        relative: "sign-in/page.tsx",
        content: r#"'use client'

import { useState } from 'react'
import { createAuthClient } from '@ruvyxa/auth/client'

const auth = createAuthClient()

export default function SignInPage() {
  const [error, setError] = useState('')
  return (
    <main>
      <h1>Sign in</h1>
      <form onSubmit={async (event) => {
        event.preventDefault()
        setError('')
        const values = new FormData(event.currentTarget)
        try {
          await auth.login('credentials', { email: values.get('email'), password: values.get('password') })
          location.assign('/')
        } catch (cause) {
          setError(cause instanceof Error ? cause.message : 'Sign in failed')
        }
      }}>
        <label>Email <input name="email" type="email" autoComplete="username" required /></label>
        <label>Password <input name="password" type="password" autoComplete="current-password" required /></label>
        <button type="submit">Sign in</button>
        {error ? <p role="alert">{error}</p> : null}
      </form>
    </main>
  )
}
"#,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_scaffold_is_atomic_on_conflict_and_force_is_explicit() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("app/form-example")).unwrap();
        fs::write(temp.path().join("app/form-example/page.tsx"), "user-owned").unwrap();
        let args = AddArgs {
            templates: vec![AddTemplate::Form],
            root: temp.path().to_path_buf(),
            runtime: None,
            force: false,
        };
        assert!(
            scaffold_add(args)
                .unwrap_err()
                .to_string()
                .contains("RUV2401")
        );
        assert!(!temp.path().join("app/form-example/action.ts").exists());
        assert_eq!(
            fs::read_to_string(temp.path().join("app/form-example/page.tsx")).unwrap(),
            "user-owned"
        );
    }

    #[test]
    fn data_table_scaffold_stays_in_a_private_route_folder() {
        let files = template_files(AddTemplate::DataTable);
        assert_eq!(files.len(), 1);
        assert!(Path::new(files[0].relative).starts_with("_components"));
        assert!(files[0].content.contains("'use client'"));
    }
}
