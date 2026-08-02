import json
from collections import defaultdict

def partition(els):
    par = {}
    def find(x):
        while par.setdefault(x, x) != x:
            par[x] = par[par[x]]; x = par[x]
        return x
    def union(a, b):
        par[find(a)] = find(b)
    for e in els:
        for p in e['pins']:
            find(tuple(p))
    G = ('G',)
    find(G)
    for e in els:
        t = e['kind']['t']
        if t == 'Wire':
            union(tuple(e['pins'][0]), tuple(e['pins'][1]))
        if t == 'Ground':
            union(tuple(e['pins'][0]), G)
    nets = defaultdict(set)
    rails = defaultdict(set)
    for e in els:
        t = e['kind']['t']
        if t == 'Rail':
            rails[find(tuple(e['pins'][0]))].add(json.dumps(e['kind'], sort_keys=True))
        if t in ('Wire', 'Ground', 'Rail'):
            continue
        for i, p in enumerate(e['pins']):
            nets[find(tuple(p))].add((e['id'], i))
    out = {}
    for root, pins in nets.items():
        g = find(G) == find(root)
        for pin in pins:
            out[pin] = (frozenset(pins), g, frozenset(rails.get(find(root), ())))
    return out

for name, (a, b) in {
    'synth': ('synth_before.json', 'synth_after.json'),
    'showcase': ('showcase_before.json', 'showcase_after.json'),
    'hoist': ('hoist_before.json', 'hoist_after.json'),
}.items():
    A = partition(json.load(open(a)))
    B = partition(json.load(open(b)))
    same = A == B
    print(name, 'part-pin partition equal (up to node renumbering):', same,
          '| pins:', len(A), '->', len(B))
    assert same

els = json.load(open('hoist_after.json'))
B = partition(els)
for pin, (members, g, r) in sorted(B.items()):
    if pin[0] >= 900:
        print('fixture pin', pin, 'net members', sorted(members), 'ground' if g else '')
