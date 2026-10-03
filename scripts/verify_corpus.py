"""Audit a downloaded/grouped corpus manifest, retaining per-command evidence.

python scripts/verify_corpus.py --manifest can_example/vector_samples/2026-10-03/manifest.json --out artifacts/vector-audit
Requires the independent oracle dependency python-can==4.6.1.
"""
import argparse
import csv
import hashlib
import json
import platform
import subprocess
import sys
import time
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT/'scripts'))
from verify_samples import key, reference_compare, native_fingerprint

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--exe', type=Path, default=ROOT/'dist/canlog.exe')
    args = parser.parse_args()
    output = args.out.resolve(); output.mkdir(parents=True, exist_ok=False)
    manifest_path = args.manifest.resolve(); source_root = manifest_path.parent
    exe = args.exe.resolve()
    manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
    all_entries = manifest.get('files')
    if all_entries is None:
        all_entries = [f for group in manifest['groups'] for f in group['files']]
    input_logs = [r for r in all_entries if Path(r['local_path']).suffix.lower() in ('.asc','.blf')
                  and r.get('role') in ('primary','log')]
    assert input_logs, 'manifest has no ASC/BLF logs'
    for item in all_entries:
        assert hashlib.sha256((source_root/item['local_path']).read_bytes()).hexdigest()==item['sha256']
    common = ['--unsupported','skip','--recover']
    allowed = []
    for loss in ['unsupported-record','corrupted-region','field:format-metadata','field:source-metadata']:
        allowed += ['--allow-loss',loss]
    commands = []; results = []; serial = 0

    def invoke(label, *arguments):
        nonlocal serial
        serial += 1
        started = time.perf_counter()
        process = subprocess.run([str(exe),*map(str,arguments)],capture_output=True,text=True,
                                 encoding='utf-8',errors='replace',timeout=40)
        log_stem = f'{serial:03d}_{label}'
        (output/(log_stem+'.stdout.txt')).write_text(process.stdout,encoding='utf-8')
        (output/(log_stem+'.stderr.txt')).write_text(process.stderr,encoding='utf-8')
        commands.append({'label':label,'argv':[str(exe),*map(str,arguments)],
                         'exit_code':process.returncode,'elapsed_s':time.perf_counter()-started,
                         'stdout':log_stem+'.stdout.txt','stderr':log_stem+'.stderr.txt'})
        return process

    def require(process):
        if process.returncode not in (0,3):
            raise AssertionError(f'exit {process.returncode}: {process.stderr[-1200:]}')
        return process

    def frames(path,label):
        process = require(invoke(label,'view',path,'--unlimited',*common))
        return [json.loads(line)['frame'] for line in process.stdout.splitlines() if line.strip()]

    for i, item in enumerate(input_logs):
        relative = item['local_path']; source = source_root/relative
        prefix = f'{i:02d}_{Path(relative).parent.name}_{source.stem}'
        row = {'path':relative,'bytes':source.stat().st_size,'sha256':item['sha256']}
        assert hashlib.sha256(source.read_bytes()).hexdigest()==item['sha256']
        invoke(prefix+'_info','info',source,*common)
        strict = invoke(prefix+'_strict_stats','stats',source)
        row['strict_scan'] = {'exit_code':strict.returncode,'diagnostic':strict.stderr[-1800:]}
        scan = invoke(prefix+'_recover_stats','stats',source,*common)
        row['scan'] = {'exit_code':scan.returncode}
        if scan.returncode not in (0,3):
            row['scan']['diagnostic'] = scan.stderr[-1800:]
            row['roundtrip'] = {'status':'not_run','reason':'source rejected by reader'}
            results.append(row)
            print(json.dumps({'path':relative,'scan':'rejected','reason':scan.stderr.strip()}),flush=True)
            continue
        stats = json.loads(scan.stdout)
        row['scan'].update({k:stats[k] for k in ['status','frames_read','issues','issue_counts','scan_complete','regressions','issue_examples']})
        original = frames(source,prefix+'_view')
        assert len(original)==stats['frames_read']
        row['original_external_reader'] = {}
        try:
            row['original_external_reader'] = {'status':'match',**reference_compare(source,original,source.suffix[1:])}
        except Exception as exc:
            row['original_external_reader'] = {'status':'difference','diagnostic':str(exc)}
        try:
            replay = require(invoke(prefix+'_replay','replay',source,'--no-wait','--on-regression','immediate',*common))
            replay_frames = [json.loads(line)['frame'] for line in replay.stdout.splitlines() if line.strip()]
            assert list(map(key,replay_frames))==list(map(key,original))
            row['replay'] = {'status':'pass','frames':len(original),'exit_code':replay.returncode}

            blf = output/(prefix+'.recorded.blf'); asc = output/(prefix+'.replayed.asc')
            recorded_report = output/(prefix+'.record-report.json')
            record = require(invoke(prefix+'_record','record','--input',source,'--output',blf,
                                    '--report',recorded_report,*common,*allowed))
            assert list(map(key,frames(blf,prefix+'_recorded_view')))==list(map(key,original))
            record_stats = json.loads(recorded_report.read_text(encoding='utf-8'))
            assert record_stats['published'] and record_stats['finalized']
            require(invoke(prefix+'_replay_file','replay',blf,'--output',asc,'--no-wait','--on-regression','immediate',*allowed))
            assert list(map(key,frames(asc,prefix+'_replayed_view')))==list(map(key,original))
            row['roundtrip'] = {'status':'pass','frames':len(original),'record_exit_code':record.returncode,
                                'actual_losses':record_stats['losses'],
                                'external_recorded_blf':reference_compare(blf,original,'blf'),
                                'external_replayed_asc':reference_compare(asc,original,'asc')}

            exported = output/(prefix+'.csv')
            require(invoke(prefix+'_export_csv','export',source,'--output',exported,*common,*allowed))
            with exported.open(encoding='utf-8',newline='') as file:
                csv_rows = list(csv.DictReader(file))
            assert len(csv_rows)==len(original)
            for csv_row,frame in zip(csv_rows,original):
                for name in ['timestamp_ns','channel','id','raw_dlc']:
                    assert int(csv_row[name])==frame[name]
                assert bytes.fromhex(csv_row['data_hex'])==bytes(frame['data'])
                assert csv_row['direction'].lower()==frame['direction']
                for name in ['extended','remote','fd','brs','esi']:
                    assert (csv_row[name].lower() in ['true','1'])==frame[name]
            row['csv_export'] = {'status':'pass','frames':len(csv_rows)}

            if original:
                first = original[0]; expected = [f for f in original if f['id']==first['id'] and
                           f['channel']==first['channel'] and f['extended']==first['extended']][:4]
                filtered = output/(prefix+'.filtered.jsonl')
                require(invoke(prefix+'_filter','filter',source,'--output',filtered,*common,*allowed,
                        '--id',hex(first['id']),'--channel',first['channel'],'--id-kind',
                        'extended' if first['extended'] else 'standard','--limit',4))
                assert list(map(key,frames(filtered,prefix+'_filter_view')))==list(map(key,expected))
                row['filter'] = {'status':'pass','selected_frames':len(expected),'id':hex(first['id']),
                                 'channel':first['channel']}
            else:
                row['filter'] = {'status':'not_run','reason':'no readable CAN frames'}
        except Exception as exc:
            row['operation_failure'] = str(exc)

        native = output/(prefix+'.native'+source.suffix)
        native_report = output/(prefix+'.native-report.json')
        preserved = invoke(prefix+'_preserve','record','--input',source,'--output',native,
                           '--preserve-records','--report',native_report)
        if preserved.returncode==0:
            try:
                native_stats = json.loads(native_report.read_text(encoding='utf-8'))
                assert native_stats['published'] and not native_stats['losses']
                assert list(map(key,frames(native,prefix+'_native_view')))==list(map(key,original))
                fingerprint = native_fingerprint(source)
                assert fingerprint==native_fingerprint(native)
                native_replay = output/(prefix+'.native_replay'+source.suffix)
                require(invoke(prefix+'_native_replay','replay',native,'--output',native_replay,
                                '--preserve-records','--no-wait','--on-regression','immediate'))
                assert fingerprint==native_fingerprint(native_replay)
                row['native_preservation'] = {'status':'pass',**fingerprint,
                            'issues_preserved':native_stats['issues_preserved']}
            except Exception as exc:
                row['native_preservation'] = {'status':'verification_failure','diagnostic':str(exc)}
        else:
            row['native_preservation'] = {'status':'rejected','exit_code':preserved.returncode,
                            'output_not_published':not native.exists(),'diagnostic':preserved.stderr[-1800:]}
        results.append(row)
        print(json.dumps({'path':relative,'frames':len(original),'scan':stats['status'],
                          'roundtrip':row.get('roundtrip',{}).get('status'),
                          'native':row['native_preservation']['status'],
                          'operation_failure':row.get('operation_failure')}),flush=True)

    for item in all_entries:
        assert hashlib.sha256((source_root/item['local_path']).read_bytes()).hexdigest()==item['sha256']
    for item in manifest.get('source_files',[]):
        source = Path(manifest['source_root'])/item['source_path']
        assert hashlib.sha256(source.read_bytes()).hexdigest()==item['sha256']
    report = {'verified_at_utc':datetime.now(timezone.utc).isoformat(),'platform':platform.platform(),'exe':str(exe),
              'exe_sha256':hashlib.sha256(exe.read_bytes()).hexdigest(),
              'manifest':str(manifest_path),
              'original_files_unchanged':len(all_entries),
              'installed_source_files_unchanged':len(manifest.get('source_files',[])),
              'scope':'Downloaded ASC/BLF only. DBC/CDD are not supported CLI inputs; no diagnostic/signal decoding claim.',
              'samples':results,'commands':commands,
              'summary':{'log_count':len(results),'scan_complete':sum(r['scan'].get('status')=='complete' for r in results),
                'scan_partial':sum(r['scan'].get('status')=='partial' for r in results),
                'scan_rejected':sum(r['scan']['exit_code'] not in (0,3) for r in results),
                'can_frames':sum(r['scan'].get('frames_read',0) for r in results),
                'nonempty_log_count':sum(r['scan'].get('frames_read',0)>0 for r in results),
                'roundtrip_pass':sum(r['roundtrip'].get('status')=='pass' for r in results if 'roundtrip' in r),
                'operation_failures':sum('operation_failure' in r for r in results),
                'native_pass':sum(r.get('native_preservation',{}).get('status')=='pass' for r in results),
                'native_verification_failures':sum(r.get('native_preservation',{}).get('status')=='verification_failure' for r in results)}}
    (output/'results.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({'report':str(output/'results.json'),'summary':report['summary']},indent=2),flush=True)
    if report['summary']['operation_failures'] or report['summary']['native_verification_failures']:
        raise SystemExit(1)

if __name__=='__main__':main()
