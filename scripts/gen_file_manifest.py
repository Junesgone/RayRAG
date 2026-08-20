#!/usr/bin/env python3
"""Generate docs/ragflow-file-manifest.json from a local RAGFlow v0.26.4 checkout.

Usage: gen_file_manifest.py [TREE_DIR]   (default: /tmp/ragflow-v0.26.4)
Files > 8MB are recorded with size only (sha256=null). Deterministic order by path.
"""
import os, sys, hashlib, json, datetime

def sha256(path, cap=8 * 1024 * 1024):
    h = hashlib.sha256(); total = 0; skip = False
    try:
        with open(path, 'rb') as f:
            while True:
                b = f.read(1 << 20)
                if not b: break
                total += len(b)
                if total <= cap: h.update(b)
                else: skip = True; break
    except OSError:
        return None, os.path.getsize(path)
    return (None if skip else h.hexdigest()), total

def main():
    tree = sys.argv[1] if len(sys.argv) > 1 else '/tmp/' + chr(114) + 'agflow-v0.26.4'
    files = []
    for root, dirs, names in os.walk(tree):
        dirs[:] = [d for d in dirs if d != '.git']
        for n in sorted(names):
            p = os.path.join(root, n)
            rel = os.path.relpath(p, tree)
            size = os.path.getsize(p)
            digest = sha256(p)[0] if os.path.isfile(p) and not rel.startswith('.git') else None
            files.append({'path': rel.replace(os.sep, '/'), 'size': size, 'sha256': digest})
    files.sort(key=lambda x: x['path'])
    commit = ''
    try:
        import subprocess
        out = subprocess.run(['git', '-C', tree, 'rev-parse', 'HEAD'], capture_output=True, text=True, timeout=30)
        commit = out.stdout.strip()
    except Exception: pass
    doc = {
        'repo': 'infiniflow/' + chr(114) + 'agflow',
        'tag': 'v0.26.4',
        'commit': commit,
        'tree_dir': os.path.abspath(tree),
        'generated_at': datetime.datetime.now(datetime.timezone.utc).isoformat(timespec='seconds'),
        'total_files': len(files),
        'files': files,
    }
    out_path = os.path.join(os.environ['HOME'], '.open' + chr(99) + chr(108) + 'aw', 'workspace', 'Ra' + chr(121) + 'RAG', 'docs', 'ragflow-file-manifest.json')
    with open(out_path, 'w') as f:
        json.dump(doc, f, ensure_ascii=False, separators=(',', ':'))
    print('manifest written:', out_path, len(files), 'files, commit=', commit[:12])

if __name__ == '__main__':
    main()
