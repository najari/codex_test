"""Verify CDD-free raw analysis/replay against the independently checked KWP corpus.

The baseline comes from verify_kwp_cdd.py. Only its transport rows are compared;
no diagnostic decoder or protocol-specific SID rules are used by this verifier.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', type=Path, required=True)
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()
    baseline = json.loads(args.baseline.read_text())
    assert baseline['assertions_passed']
    exe = args.exe.resolve()
    args.artifacts.mkdir(parents=True, exist_ok=False)
    evidence = {'binary': str(exe), 'binary_sha256': sha(exe),
                'baseline': str(args.baseline), 'baseline_sha256': sha(args.baseline),
                'originals': baseline['originals'], 'commands': [], 'cases': []}
    for source, fingerprint in evidence['originals'].items():
        assert sha(Path(source)) == fingerprint

    def transport(rows):
        return [{k: v for k, v in row.items()
                 if k not in ('analyzer', 'timeline_key', 'result_key')}
                for row in rows if row['kind'] in ('payload', 'flow_control', 'protocol_issue')]

    for case in baseline['cases']:
        name = case['name']
        original = next(c['argv'] for c in baseline['commands']
                        if c['argv'][0] == 'kwp' and '--dbc' not in c['argv']
                        and Path(c['argv'][1]).stem == name)
        policy = json.loads(Path(original[original.index('--policy') + 1]).read_text())
        for route in policy['routes']:
            route.pop('cdd', None)
        raw_policy = args.artifacts / f'{name}.policy.json'
        raw_policy.write_text(json.dumps(policy))
        gold = [json.loads(line) for line in
                Path(original[original.index('-o') + 1]).read_text().splitlines()]
        analysis = None
        for mode in ('kwp', 'replay'):
            output = args.artifacts / f'{name}.{mode}.jsonl'
            report = args.artifacts / f'{name}.{mode}.report.json'
            command = [mode, original[1], '--routes', original[original.index('--routes') + 1],
                       '--policy', str(raw_policy), '-o', str(output), '--report', str(report)]
            if mode == 'replay':
                command += ['--protocol', 'kwp2000-vector', '--no-wait']
            proc = subprocess.run([str(exe), *command], capture_output=True)
            evidence['commands'].append({'argv': command, 'exit': proc.returncode,
                                         'stderr': proc.stderr.decode('utf-8', errors='replace')})
            assert proc.returncode == 0, evidence['commands'][-1]
            rows = [json.loads(line) for line in output.read_text().splitlines()]
            assert rows == [r for r in rows if not r['kind'].startswith('kwp')]
            assert transport(rows) == transport(gold), (name, mode)
            summary = json.loads(report.read_text())
            assert summary['kwp_counts'] == {} and summary.get('cdd_counts') is None
            assert summary['scan_complete'] and summary['published'] and summary['status'] == 'complete'
            if mode == 'kwp':
                analysis = output.read_bytes()
            else:
                assert output.read_bytes() == analysis, name
            evidence['cases'].append({'name': name, 'mode': mode, 'rows': len(rows),
                                      'raw_payloads': sum(r['kind'] == 'payload' for r in rows)})
    for source, fingerprint in evidence['originals'].items():
        assert sha(Path(source)) == fingerprint
    assert sha(exe) == evidence['binary_sha256']
    evidence['passed'] = True
    (args.artifacts / 'results.json').write_text(json.dumps(evidence, indent=2), encoding='utf-8')
    print(json.dumps({'passed': True, 'cases': len(evidence['cases'])}, indent=2))


if __name__ == '__main__':
    main()
