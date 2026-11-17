#!/usr/bin/env python3
"""Per-transcript agent build cost. Read-only. usage: cost.py name=path.jsonl ...
Price parameters ($ per million tokens) are arguments, not facts: --in --out --cw5m --cw1h --cr."""
import json, sys, re, datetime, collections, argparse
ap = argparse.ArgumentParser()
ap.add_argument('files', nargs='+')
ap.add_argument('--in', dest='p_in', type=float, default=5.0)
ap.add_argument('--out', dest='p_out', type=float, default=25.0)
ap.add_argument('--cw5m', type=float, default=None, help='default 1.25 x input')
ap.add_argument('--cw1h', type=float, default=None, help='default 2 x input')
ap.add_argument('--cr', type=float, default=None, help='default 0.1 x input')
ap.add_argument('--idle', type=float, default=600, help='gaps longer than this (s) after a rate-limit message or user wait are excluded')
a = ap.parse_args()
cw5 = a.cw5m if a.cw5m is not None else 1.25 * a.p_in
cw1 = a.cw1h if a.cw1h is not None else 2.0 * a.p_in
cr = a.cr if a.cr is not None else 0.1 * a.p_in
def ts(s): return datetime.datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp()
CARGO = re.compile(r'\bcargo\s+(?:\+\S+\s+)?(build|check|clippy|run|test|tree|metadata|update|fmt|clean|doc|add)\b')
for spec in a.files:
    name, path = spec.split('=', 1)
    msgs = {}            # message.id -> usage (dedupe: one API response is split over several lines)
    models = collections.Counter()
    tools = collections.Counter()
    tool_inputs = {}     # tool_use id -> (name, input)
    results = {}         # tool_use id -> (is_error, text)
    times = []
    synth = []
    prompts = 0
    last_cost = None
    for line in open(path):
        try: d = json.loads(line)
        except ValueError: continue
        t = d.get('timestamp')
        if t and d.get('type') in ('user', 'assistant'): times.append(ts(t))
        if d.get('type') == 'cost-state': last_cost = d
        m = d.get('message') if isinstance(d.get('message'), dict) else None
        if not m: continue
        if d.get('type') == 'assistant':
            if m.get('model') == '<synthetic>':
                synth.append(ts(t)); continue
            mid = m.get('id') or d.get('requestId')
            if m.get('usage'): msgs[mid] = m['usage']
            models[m.get('model')] += 0  # count models per response below
            for c in m.get('content') or []:
                if isinstance(c, dict) and c.get('type') == 'tool_use':
                    tools[c['name']] += 1
                    tool_inputs[c['id']] = (c['name'], c.get('input') or {})
        elif d.get('type') == 'user':
            c = m.get('content')
            if isinstance(c, list):
                tr = [x for x in c if isinstance(x, dict) and x.get('type') == 'tool_result']
                for x in tr:
                    body = x.get('content')
                    if isinstance(body, list): body = '\n'.join(y.get('text', '') for y in body if isinstance(y, dict))
                    results[x['tool_use_id']] = (bool(x.get('is_error')), body or '')
                if not tr and not d.get('isMeta'): prompts += 1
            elif isinstance(c, str) and not d.get('isMeta'):
                prompts += 1
    # tokens
    tok = collections.Counter()
    for u in msgs.values():
        tok['input'] += u.get('input_tokens', 0)
        tok['output'] += u.get('output_tokens', 0)
        tok['thinking(of output)'] += (u.get('output_tokens_details') or {}).get('thinking_tokens', 0)
        cc = u.get('cache_creation') or {}
        c5, c1 = cc.get('ephemeral_5m_input_tokens', 0), cc.get('ephemeral_1h_input_tokens', 0)
        if not cc: c5 = u.get('cache_creation_input_tokens', 0)
        tok['cache_write_5m'] += c5; tok['cache_write_1h'] += c1
        tok['cache_read'] += u.get('cache_read_input_tokens', 0)
    cost = (tok['input'] * a.p_in + tok['output'] * a.p_out + tok['cache_write_5m'] * cw5 + tok['cache_write_1h'] * cw1 + tok['cache_read'] * cr) / 1e6
    # wall clock: total span, minus gaps that follow a rate-limit message
    times.sort(); span = times[-1] - times[0]
    excluded = 0.0
    for s in synth:
        nxt = [x for x in times if x > s + 1]
        if nxt and nxt[0] - s > a.idle: excluded += nxt[0] - s
    # cargo
    builds = fails = errs = 0; kinds = collections.Counter(); err_codes = collections.Counter()
    for tid, (nm, inp) in tool_inputs.items():
        if nm != 'Bash': continue
        cmd = inp.get('command', '')
        ks = CARGO.findall(cmd)
        if not any(k in ('build', 'check', 'clippy', 'run', 'test') for k in ks): continue
        builds += 1
        for k in set(ks): kinds[k] += 1
        is_err, body = results.get(tid, (False, ''))
        n = len(re.findall(r'^error(\[E\d+\])?: ', body, re.M))
        n -= len(re.findall(r'^error: (could not compile|aborting due to)', body, re.M))
        for code in re.findall(r'^error\[(E\d+)\]', body, re.M): err_codes[code] += 1
        if re.search(r'could not compile|^error(\[E\d+\])?: ', body, re.M) or (is_err and 'error' in body.lower()):
            fails += 1
        errs += max(n, 0)
    loc_added = 0
    for tid, (nm, inp) in tool_inputs.items():
        if nm == 'Write': loc_added += inp.get('content', '').count('\n') + 1
        elif nm == 'Edit': loc_added += inp.get('new_string', '').count('\n') + 1 - (inp.get('old_string', '').count('\n') + 1)
    print(f'== {name}')
    print(f'  API responses (deduped): {len(msgs)}; user prompts: {prompts}; rate-limit stops: {len(synth)}')
    print('  tokens: ' + ', '.join(f'{k}={v:,}' for k, v in tok.items()))
    print(f'  cost at in={a.p_in} out={a.p_out} cw5m={cw5} cw1h={cw1} cr={cr} $/MTok: ${cost:,.2f}' + (f'  (harness cost-state: ${last_cost["totalCostUSD"]:.2f}, API time {last_cost["totalAPIDuration"]/60000:.1f} min, lines +{last_cost["totalLinesAdded"]}/-{last_cost["totalLinesRemoved"]})' if last_cost else ''))
    print(f'  wall: span {span/3600:.2f} h, minus rate-limit pauses {excluded/3600:.2f} h = active {(span-excluded)/60:.1f} min')
    print(f'  tool calls: {sum(tools.values())} ' + str(dict(tools.most_common())))
    print(f'  cargo build/check/clippy/run/test invocations: {builds} {dict(kinds)}; with compile errors: {fails}; error diagnostics: {errs}; top codes {err_codes.most_common(6)}')
    print(f'  net lines written via Write/Edit (all files, approx): {loc_added}')
