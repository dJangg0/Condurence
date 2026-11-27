/** One visible line of the file tree. */
export type TreeRow = {
  kind: "dir" | "file";
  /** Workspace-relative path with `/` separators. */
  path: string;
  name: string;
  depth: number;
  /** For a directory: its children are listed below it. */
  open: boolean;
};

type Node = { dirs: Map<string, Node>; files: string[] };

const byName = (a: string, b: string) => a.localeCompare(b, undefined, { sensitivity: "base", numeric: true });

/** The directories above a file, outermost first: `src/lib/a.ts` has `src` and `src/lib`. */
export function ancestorDirs(path: string): string[] {
  const parts = path.split("/");
  const dirs: string[] = [];
  for (let end = 1; end < parts.length; end += 1) dirs.push(parts.slice(0, end).join("/"));
  return dirs;
}

/** Every directory that holds at least one of the paths, however deep. */
export function dirsHolding(paths: Iterable<string>): Set<string> {
  const dirs = new Set<string>();
  for (const path of paths) for (const dir of ancestorDirs(path)) dirs.add(dir);
  return dirs;
}

/**
 * The rows to draw: directories before files at each level, each group by name, and the contents of
 * a directory only when it is in `open`. Paths are the listing's, so there is no filesystem access.
 */
export function treeRows(files: readonly string[], open: ReadonlySet<string>): TreeRow[] {
  const root: Node = { dirs: new Map(), files: [] };
  for (const path of files) {
    const parts = path.split("/");
    let node = root;
    for (const part of parts.slice(0, -1)) {
      let next = node.dirs.get(part);
      if (!next) {
        next = { dirs: new Map(), files: [] };
        node.dirs.set(part, next);
      }
      node = next;
    }
    node.files.push(path);
  }

  const rows: TreeRow[] = [];
  const walk = (node: Node, prefix: string, depth: number) => {
    for (const name of [...node.dirs.keys()].sort(byName)) {
      const path = prefix ? `${prefix}/${name}` : name;
      const isOpen = open.has(path);
      rows.push({ kind: "dir", path, name, depth, open: isOpen });
      if (isOpen) walk(node.dirs.get(name) as Node, path, depth + 1);
    }
    for (const path of [...node.files].sort((a, b) => byName(baseName(a), baseName(b)))) {
      rows.push({ kind: "file", path, name: baseName(path), depth, open: false });
    }
  };
  walk(root, "", 0);
  return rows;
}

export function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/**
 * Files matching a search box. Every word has to appear in the path, ignoring case. Files whose
 * name carries the first word come first, then the shorter paths.
 */
export function matchFiles(files: readonly string[], query: string, limit = 200): string[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [];
  const hits = files.filter((path) => {
    const lower = path.toLowerCase();
    return words.every((word) => lower.includes(word));
  });
  const nameHit = (path: string) => (baseName(path).toLowerCase().includes(words[0]) ? 0 : 1);
  return hits.sort((a, b) => nameHit(a) - nameHit(b) || a.length - b.length || byName(a, b)).slice(0, limit);
}
