#!/usr/bin/env python3
"""Read-only audit of saved Drive metadata against current local Git ignore rules."""
import argparse
from collections import defaultdict, deque, Counter
import csv
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile


def private_file(path):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    return os.fdopen(fd, 'w', encoding='utf-8', newline='')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--discovery', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    discovery = json.loads(args.discovery.read_text())
    root = Path(discovery['root'])
    valid = set(discovery['valid_repositories'])
    contexts = {}
    # Every real repository is independent. Other .gitignore trees use an isolated,
    # empty Git index; these cannot prove whether a file used to be tracked.
    for p in discovery['repositories']:
        contexts[str(Path(p).relative_to(root))] = {'root':p, 'tracked_checked':p in valid}
    for p in sorted(discovery['gitignore_files'], key=lambda x: len(Path(x).parts)):
        parent = Path(p).parent
        if not any(parent == Path(c['root']) or Path(c['root']) in parent.parents
                   for c in contexts.values()):
            contexts[str(parent.relative_to(root))] = {'root':str(parent), 'tracked_checked':False}
    ordered = sorted(contexts, key=len, reverse=True)
    def context_for(path):
        return next((c for c in ordered if path == c or path.startswith(c + '/')), None)
    env = os.environ.copy()
    for key in list(env):
        if key.startswith('GIT_'):
            env.pop(key)
    env['GIT_OPTIONAL_LOCKS'] = '0'
    grouped = defaultdict(list)
    counts = Counter()
    coverage = {'discovery_errors':discovery['errors'], 'invalid_repositories':discovery['invalid_repositories'],
                'unmapped_items':0, 'unsafe_names':0, 'git_errors':[], 'remote_ignore_files_missing_locally':[]}
    db = sqlite3.connect('file:' + str(args.manifest.resolve()) + '?mode=ro', uri=True)
    db.execute('BEGIN')
    source = json.loads(db.execute("SELECT json FROM state WHERE key='source'").fetchone()[0])
    progress = json.loads(db.execute("SELECT json FROM state WHERE key='progress'").fetchone()[0])
    assert Path(source['local_root']) == root
    assert progress['phase'] == 'ready', 'A completed snapshot is required'
    folders = {}; children = defaultdict(list)
    query = "SELECT id,json_extract(json,'$.name'),json_extract(json,'$.parents') FROM remote WHERE json_extract(json,'$.kind')='folder' AND json_extract(json,'$.trashed')=0"
    for id_, name, parents in db.execute(query):
        folders[id_] = name
        for parent in json.loads(parents):children[parent].append(id_)
    folder_paths = {source['remote_root_id']:('',True)}
    pending = deque([source['remote_root_id']])
    while pending:
        parent = pending.popleft(); prefix, safe = folder_paths[parent]
        for id_ in children[parent]:
            if id_ in folder_paths:continue
            name = folders[id_]
            safe_name = name not in ('', '.', '..') and '/' not in name and '\\' not in name and '\0' not in name
            path = prefix + '/' + name if prefix else name
            folder_paths[id_] = (path, safe and safe_name);pending.append(id_)
    records = db.execute("SELECT id,json_extract(json,'$.name'),json_extract(json,'$.kind'),json_extract(json,'$.fingerprint.size'),json_extract(json,'$.version'),json_extract(json,'$.parents') FROM remote WHERE json_extract(json,'$.trashed')=0")
    for id_, name, kind, size, version, parents_json in records:
        if id_ == source['remote_root_id']:continue
        if kind == 'folder':
            resolved = folder_paths.get(id_)
        else:
            resolved = None
            for parent in json.loads(parents_json):
                if parent in folder_paths:
                    prefix, safe = folder_paths[parent]
                    safe_name = name not in ('', '.', '..') and '/' not in name and '\\' not in name and '\0' not in name
                    resolved = (prefix + '/' + name if prefix else name, safe and safe_name);break
        if not resolved:
            coverage['unmapped_items'] += 1;continue
        path, safe = resolved
        counts['reachable_items'] += 1
        if not safe:coverage['unsafe_names'] += 1
        if name == '.gitignore' and not (root/path).is_file():
            coverage['remote_ignore_files_missing_locally'].append({'path':path,'remote_id':id_})
        context = context_for(path)
        if context is None:
            counts['outside_ignore_contexts'] += 1;continue
        relative = path[len(context)+1:] if path != context else ''
        if not relative or '.git' in relative.split('/'):
            counts['git_metadata_or_context_roots'] += 1;continue
        grouped[context].append((relative + ('/' if kind == 'folder' else ''),path,id_,kind,size,version,safe))
    db.close()
    print(json.dumps({'stage':'classifying','contexts':len(grouped),'items':sum(map(len,grouped.values()))}),flush=True)
    headers = ['path','remote_id','kind','size_bytes','version','repository','rule_source','rule_line','rule_pattern','tracked_index_checked','portable_path','local_exists']
    handles = {}; writers = {}
    for name in ['gitignored-files.csv','gitignored-folders.csv','other-git-exclusions.csv','unverified-rule-matches.csv']:
        handles[name] = private_file(args.output/name);writers[name] = csv.writer(handles[name]);writers[name].writerow(headers)
    jsonl = private_file(args.output/'candidates.jsonl')
    by_context = defaultdict(Counter)
    fingerprints = {}
    for file in discovery['gitignore_files']:
        try:fingerprints[file] = hashlib.sha256(Path(file).read_bytes()).hexdigest()
        except OSError as error:coverage['git_errors'].append({'path':file,'error':str(error)})
    with tempfile.TemporaryDirectory(prefix='freesync-ignore-audit-') as temp:
        virtual = Path(temp)/'empty.git'
        subprocess.run(['git','init','--bare','--quiet',str(virtual)],check=True,env=env)
        for context, entries in sorted(grouped.items()):
            info = contexts[context]
            if info['tracked_checked']:
                command = ['git','-C',info['root'],'check-ignore','-v','-z','--stdin']
            else:
                command = ['git','--git-dir='+str(virtual),'--work-tree='+info['root'],'check-ignore','--no-index','-v','-z','--stdin']
            def check_batch(batch):
                query = sorted({e[0] for e in batch})
                result = subprocess.run(command,input=('\0'.join(query)+'\0').encode(),capture_output=True,env=env,cwd=info['root'])
                if result.returncode not in (0,1):
                    if len(query)>1:
                        middle=len(batch)//2
                        if middle:
                            return {**check_batch(batch[:middle]),**check_batch(batch[middle:])}
                    coverage['git_errors'].append({'repository':context,'query':query[:1],'error':result.stderr.decode(errors='replace')[:1000]})
                    return {}
                fields=result.stdout.decode().split('\0');assert fields[-1]==''
                matched={}
                for offset in range(0,len(fields)-1,4):
                    rule_source,line,pattern,path=fields[offset:offset+4]
                    if not pattern.startswith('!'):matched[path]=(rule_source,line,pattern)
                return matched
            for start in range(0,len(entries),4096):
                batch=entries[start:start+4096];matched=check_batch(batch)
                for query,path,id_,kind,size,version,safe in batch:
                    if query not in matched:continue
                    rule_source,line,pattern=matched[query]
                    source_path=Path(rule_source)
                    is_gitignore=source_path.name=='.gitignore'
                    name=('unverified-rule-matches.csv' if not info['tracked_checked'] else
                          'other-git-exclusions.csv' if not is_gitignore else
                          'gitignored-folders.csv' if kind=='folder' else 'gitignored-files.csv')
                    row=[path,id_,kind,size,version,context,rule_source,line,pattern,info['tracked_checked'],safe,os.path.lexists(root/path)]
                    writers[name].writerow(row)
                    record=dict(zip(headers,row));record['classification']=name.removesuffix('.csv')
                    jsonl.write(json.dumps(record,ensure_ascii=False)+'\n')
                    counts[name] += 1;by_context[context][name] += 1
                    if kind!='folder' and size is not None:counts[name+':bytes'] += size
            print(json.dumps({'repository':context,'checked':len(entries),'matched':sum(by_context[context].values())}),flush=True)
    jsonl.close()
    for handle in handles.values():handle.close()
    with private_file(args.output/'by-repository.csv') as output:
        writer=csv.writer(output);writer.writerow(['repository','ignored_files','ignored_folders','other_git_exclusions','unverified_matches'])
        for context,c in sorted(by_context.items(),key=lambda x:sum(x[1].values()),reverse=True):
            writer.writerow([context,c['gitignored-files.csv'],c['gitignored-folders.csv'],c['other-git-exclusions.csv'],c['unverified-rule-matches.csv']])
    changed=[]
    for path,digest in fingerprints.items():
        try:
            if hashlib.sha256(Path(path).read_bytes()).hexdigest()!=digest:changed.append(path)
        except OSError:changed.append(path)
    coverage['ignore_files_changed_during_audit']=changed
    summary={'created_at':datetime.now(timezone.utc).isoformat(),'snapshot_completed_at':progress['completed_at'],
             'snapshot_revision':progress['revision'],'root':str(root),'remote_root_id':source['remote_root_id'],
             'counts':dict(counts),'repositories_discovered':len(discovery['repositories']),
             'valid_repositories':len(valid),'gitignore_files':len(discovery['gitignore_files']),
             'coverage':coverage,'rules_sha256':fingerprints,
             'policy':'Current local Git rules and index; no cloud mutations. Unknown tracking is separate. Folder matches do not authorize recursive deletion.'}
    with private_file(args.output/'summary.json') as output:json.dump(summary,output,ensure_ascii=False,indent=2)
    with private_file(args.output/'README.txt') as output:
        output.write('READ-ONLY REMOTE GIT-IGNORE AUDIT\n\n')
        output.write('Source: completed Drive metadata snapshot; rules: current local Git ignore files.\n')
        output.write('No remote files were downloaded, modified, moved or deleted.\n\n')
        output.write('gitignored-files.csv: .gitignore matches, excluding locally tracked files.\n')
        output.write('gitignored-folders.csv: matching directories; not recursive-deletion approvals.\n')
        output.write('other-git-exclusions.csv: global Git or .git/info/exclude matches.\n')
        output.write('unverified-rule-matches.csv: matches where tracking could not be checked.\n')
        output.write('candidates.jsonl: machine-readable records including opaque Drive IDs.\n')
        output.write('by-repository.csv: grouped counts. summary.json: provenance and coverage limitations.\n\n')
        output.write('Before cleanup, revalidate remote identity, versions, ancestry, current rules and folder descendants.\n')
        output.write('Missing local .gitignore files and Git-query failures are listed in summary.json.\n')
    print(json.dumps({'report':str(args.output),'counts':dict(counts),'git_errors':len(coverage['git_errors']),'missing_local_ignore_files':len(coverage['remote_ignore_files_missing_locally']),'changed_ignore_files':len(changed)}),flush=True)


if __name__=='__main__':main()
