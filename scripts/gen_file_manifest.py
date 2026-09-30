#!/usr/bin/env python3
"""Generate docs/ragflow-file-manifest.json from a local RAGFlow v0.27.2 checkout.

Usage: gen_file_manifest.py [TREE_DIR] [TAG]   (default: ~/.cache/ragflow-v0.27.2 v0.27.2)
Files > 8MB are recorded with size only (sha256=null). Deterministic order by path.
"""

import datetime
import hashlib
import json
import os
import subprocess
import sys

HASH_CAP = 8 * 1024 * 1024


def sha256(path):
    """Hash at most HASH_CAP bytes; oversized files return (None, bytes read)."""
    digest = hashlib.sha256()
    total = 0
    try:
        with open(path, 'rb') as handle:
            while True:
                block = handle.read(1 << 20)
                if not block:
                    break
                total += len(block)
                if total > HASH_CAP:
                    return None, total
                digest.update(block)
    except OSError as exc:
        print(f"warning: cannot read {path}: {exc}", file=sys.stderr)
        return None, None
    return digest.hexdigest(), total


def tree_commit(tree):
    try:
        result = subprocess.run(
            ['git', '-C', tree, 'rev-parse', 'HEAD'],
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
    except OSError as exc:
        print(f"warning: cannot read the tree commit: {exc}", file=sys.stderr)
        return ''
    return result.stdout.strip()


def main():
    tag = sys.argv[2] if len(sys.argv) > 2 else 'v0.27.2'
    home = os.environ.get('HOME') or '.'
    default_tree = os.path.join(home, '.cache', 'ragflow-v0.27.2')
    tree = sys.argv[1] if len(sys.argv) > 1 else default_tree
    files = []
    for root, dirs, names in os.walk(tree):
        dirs[:] = [name for name in dirs if name != '.git']
        for name in sorted(names):
            path = os.path.join(root, name)
            relative = os.path.relpath(path, tree)
            try:
                size = os.path.getsize(path)
            except OSError:
                # Dangling symlinks in the upstream tree are not real entries.
                continue
            digest = None
            if os.path.isfile(path) and not relative.startswith('.git'):
                digest = sha256(path)[0]
            files.append({'path': relative.replace(os.sep, '/'), 'size': size, 'sha256': digest})
    files.sort(key=lambda item: item['path'])
    document = {
        'repo': 'infiniflow/ragflow',
        'tag': tag,
        'commit': tree_commit(tree),
        'tree_dir': os.path.abspath(tree),
        'generated_at': datetime.datetime.now(datetime.timezone.utc).isoformat(timespec='seconds'),
        'total_files': len(files),
        'files': files,
    }
    out_path = os.path.join(
        os.environ['HOME'],
        '.openclaw', 'workspace', 'RayRAG',
        'docs', 'ragflow-file-manifest.json',
    )
    try:
        with open(out_path, 'w') as handle:
            json.dump(document, handle, ensure_ascii=False, separators=(',', ':'))
    except OSError as exc:
        print(f"manifest write failed: {exc}", file=sys.stderr)
        return 1
    print('manifest written:', out_path, len(files), 'files, commit=', document['commit'][:12])
    return 0


if __name__ == '__main__':
    sys.exit(main())
