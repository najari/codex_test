"""Read-only Vector CANSystem corpus verification; Python packages are test-only.

Compares ISO-TP payloads with can-isotp, DBC recognition with cantools and
CDD field placement with cantools plus explicitly declared BCD conversion.
No sample is generated or changed. Partial results are checked as partial.
"""
import argparse
import collections
import importlib.metadata
import json
from pathlib import Path
import subprocess
import xml.etree.ElementTree as ET

import can
import cantools
from verify_isotp import oracle, sha


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--exe', type=Path, default=Path('dist/canlog.exe'))
    ap.add_argument('--samples', type=Path, required=True)
    ap.add_argument('--artifacts', type=Path, required=True)
    ap.add_argument('--routes', type=Path, default=Path('examples/isotp-physical.routes.json'))
    ap.add_argument('--policy', type=Path, default=Path('examples/kwp-cansystem.policy.json'))
    args = ap.parse_args()
    args.artifacts.mkdir(parents=True, exist_ok=False)
    versions = {p: importlib.metadata.version(p) for p in ('python-can', 'can-isotp', 'cantools')}
    assert versions == {'python-can': '4.6.1', 'can-isotp': '2.0.7', 'cantools': '43.0.2'}, versions
    exe = args.exe.resolve()
    routes = json.loads(args.routes.read_text())['routes']
    evidence = {'binary': str(exe), 'binary_sha256': sha(exe), 'versions': versions,
                'commands': [], 'cases': [], 'combined': [], 'originals': {},
                'oracle_limits': 'cantools treats BCD as unsigned bits; decimal BCD conversion is independently applied only for enc=bcd CDD fields'}

    def original(path):
        evidence['originals'][str(path.resolve())] = sha(path)

    def run(command, expected):
        command = list(map(str, command))
        proc = subprocess.run([str(exe), *command], capture_output=True)
        item = {'argv': command, 'exit': proc.returncode,
                'stderr': proc.stderr.decode('utf-8', errors='replace')}
        evidence['commands'].append(item)
        assert proc.returncode == expected, item

    for path in (args.routes, args.policy):
        original(path)
    assignments = json.loads(args.policy.read_text())['routes']
    for assignment in assignments:
        configured = (args.policy.parent / assignment['cdd']['path']).resolve()
        original(configured)

    cases = [('ComfortDiagData', 1, 'comfort-demo', 'Comfort', 'CANSystemDoor', 7),
             ('EngineDiagData', 2, 'engine-demo', 'PowerTrain', 'CANSystem', 5),
             ('DiagDataA', 1, 'comfort-demo', 'Comfort', 'CANSystemDoor', 7)]
    for name, channel, route_name, dbc_name, cdd_name, positive_count in cases:
        source = args.samples / f'asc/Logging/{name}.asc'
        dbc = args.samples / f'dbc/CANdb/{dbc_name}.dbc'
        cdd = args.samples / f'cdd/CDD/{cdd_name}.cdd'
        configured = next(a for a in assignments if a['route'] == route_name)
        assert (args.policy.parent / configured['cdd']['path']).resolve() == cdd.resolve(), 'policy/corpus mismatch'
        for path in (source, dbc, cdd):
            original(path)
        with can.ASCReader(source) as reader:
            messages = list(reader)
        for message in messages:
            message.channel += 1  # python-can channels are zero-based.
        payloads = oracle(messages, routes)
        output = args.artifacts / f'{name}.jsonl'
        report = args.artifacts / f'{name}.report.json'
        run(['kwp', source, '--routes', args.routes, '--policy', args.policy,
             '-o', output, '--report', report], 3)
        rows = [json.loads(line) for line in output.read_text().splitlines()]
        actual = collections.defaultdict(list)
        for row in rows:
            event = row
            if event.get('kind') == 'payload':
                actual[event['route'], event['direction']].append(event['data_hex'])
        assert dict(actual) == payloads, (name, actual, payloads)
        transactions = [r for r in rows if r.get('kind') == 'kwp_transaction']
        positives = [r for r in transactions if r['status'] == 'positive']
        assert len(positives) == positive_count, name
        assert all(r['protocol'] == 'kwp2000_vector' for r in transactions)
        assert all(r['header']['suppress_positive_response'] is False for r in transactions)
        if name == 'EngineDiagData':
            missing = collections.Counter(r['status'] for r in transactions if r['request']['data_hex'] == '3E01')
            assert missing == {'no_response_observed': 17, 'incomplete': 1}, missing
            assert next(r for r in positives if r['request']['data_hex'] == '1081')['header']['service_id'] == 0x10

        # Undeclared session/reset messages stay visible, without the old header fallback.
        issues = [r for r in rows if r.get('kind') == 'kwp_issue']
        if name in ('ComfortDiagData', 'DiagDataA'):
            expected = {'unsupported': 2, 'orphan': 2} if name == 'ComfortDiagData' else {'unsupported': 1, 'orphan': 1}
            assert dict(collections.Counter(r['status'] for r in issues)) == expected
            assert all(r['cdd']['status'] == 'no_match' for r in issues)
        assert all(r['header']['service_key'] for r in transactions)

        definitions = cantools.database.load_file(str(cdd), database_format='cdd')
        tree = ET.parse(cdd).getroot()
        by_id = {e.get('id'): e for e in tree.iter() if e.get('id')}
        bcd_names = {e.findtext('QUAL') for e in tree.iter('DATAOBJ')
                     if (dt := by_id.get(e.get('dtref'))) is not None
                     and (cv := dt.find('CVALUETYPE')) is not None and cv.get('enc') == 'bcd'}
        field_checks = []
        for row in positives:
            request = bytes.fromhex(row['request']['data_hex'])
            if request[0] != 0x1a:
                continue
            decoded = row['cdd']
            assert decoded['status'] == 'decoded', (name, request.hex(), decoded)
            assert decoded['response']['provenance']['profile_id'] == 'uds-candela-2x'
            assert decoded['response']['provenance']['protocol'] == 'kwp2000'
            did = definitions.get_did_by_identifier(request[1])
            values = did.decode(bytes.fromhex(row['response']['data_hex'])[2:])
            native = {f['key'].split('/')[-1]: f['raw']['value'] for f in decoded['response']['fields']}
            for field, value in values.items():
                if field in bcd_names:
                    digits = format(value, 'x')
                    assert all(d in '0123456789' for d in digits), (field, value)
                    value = int(digits, 10)
                assert native[field] == str(value), (name, field, native[field], value)
                field_checks.append({'field': field, 'expected': str(value), 'actual': native[field]})
        if name == 'EngineDiagData':
            dtc = [r for r in transactions if r['request']['data_hex'] == '1802FF00']
            assert len(dtc) == 2 and all(r['status'] == 'ambiguous' and r['cdd']['status'] == 'unverified' for r in dtc), dtc
            assert all(r['reason'] == 'cdd_response_not_fully_decoded' for r in dtc)

        dbc_output = args.artifacts / f'{name}.dbc.jsonl'
        run(['decode', source, '--dbc', f'{channel}={dbc}', '-o', dbc_output], 0)
        frames = [json.loads(line) for line in dbc_output.read_text().splitlines()]
        dbc_db = cantools.database.load_file(str(dbc), strict=False)
        assert len(frames) == len(messages)
        for frame, message in zip(frames, messages):
            definition = dbc_db.get_message_by_frame_id(message.arbitration_id)
            assert frame['message'] == definition.name, (name, frame)
            assert frame['signals'] == [] and definition.signals == [], 'diagnostic transport definitions have no DBC signal values'
        summary = json.loads(report.read_text())
        assert summary['scan_complete'] and summary['published']
        evidence['cases'].append({'name': name, 'frames': len(messages), 'payloads': sum(map(len, payloads.values())),
                                  'kwp_counts': summary['kwp_counts'], 'cdd_counts': summary['cdd_counts'],
                                  'field_checks': field_checks, 'dbc_frames': len(frames), 'dbc_signal_values': 0})
        if name in ('ComfortDiagData', 'EngineDiagData'):
            sample = 'comfort' if channel == 1 else 'engine'
            config_root = Path('examples/asc-dbc-cdd')
            single_routes = config_root / f'{sample}.routes.json'
            single_policy = config_root / f'{sample}.policy.json'
            for path in (single_routes, single_policy):
                original(path)
            combined_output = args.artifacts / f'{name}.combined.jsonl'
            combined_report = args.artifacts / f'{name}.combined.report.json'
            run(['kwp', source, '--dbc', f'{channel}={dbc}', '--cdd', cdd,
                 '--ecu', 'Any_ECU_example', '--variant', 'COMMON_DIAGNOSTICS', '--allow-experimental',
                 '--routes', single_routes, '--policy', single_policy,
                 '-o', combined_output, '--report', combined_report], 3)
            combined_rows = [json.loads(line) for line in combined_output.read_text().splitlines()]
            combined_frames = [r for r in combined_rows if r.get('kind') == 'decoded_frame']
            assert [{k: v for k, v in r.items() if k not in ('analyzer', 'timeline_key', 'result_key')}
                    for r in combined_frames] == frames, name
            combined_transactions = [r for r in combined_rows if r.get('kind') == 'kwp_transaction']

            def meaning(row):
                return {k: v for k, v in row.items()
                        if k not in ('timeline_key', 'result_key', 'transaction_key', 'candidate_transaction_keys')}

            assert list(map(meaning, combined_transactions)) == list(map(meaning, transactions)), name
            frame_locations = {r['record']['location']['ordinal']: r['record']['location']
                               for r in combined_frames}
            for transaction in combined_transactions:
                for direction in ('request', 'response'):
                    if transaction[direction] is not None:
                        for location in transaction[direction]['data_locations']:
                            assert frame_locations[location['ordinal']] == location, name
            assert len({r['timeline_key'] for r in combined_rows}) == 1, name
            combined_summary = json.loads(combined_report.read_text())
            assert combined_summary['dbc_counts'] == {'decoded': len(messages)}, name
            assert combined_summary['kwp_counts'] == summary['kwp_counts'], name
            assert combined_summary['cdd_counts'] == summary['cdd_counts'], name
            assert combined_summary['scan_complete'] and combined_summary['published'], name
            evidence['combined'].append({'name': name, 'rows': len(combined_rows),
                                         'dbc_frames': len(combined_frames), 'transactions': len(combined_transactions),
                                         'output': str(combined_output), 'report': str(combined_report)})
    for path, fingerprint in evidence['originals'].items():
        assert sha(Path(path)) == fingerprint, f'original changed: {path}'
    assert sha(exe) == evidence['binary_sha256'], 'executable changed during verification'
    evidence['assertions_passed'] = True
    (args.artifacts / 'results.json').write_text(json.dumps(evidence, ensure_ascii=False, indent=2), encoding='utf-8')
    print(json.dumps({'passed': True, 'cases': len(evidence['cases']),
                      'combined': len(evidence['combined']),
                      'commands': len(evidence['commands']), 'binary_sha256': evidence['binary_sha256']}, indent=2))


if __name__ == '__main__':
    main()
