// An invented boss guide in the redesign's shapes (titled items, phases,
// mechanics, missions, HP counts, tile figures), route-injected over a real
// knowledge read so every guide field renders in tests. No real guide text.

export const GUIDE_DOC = {
  boss: 'MaleficStar',
  summary: 'Invented long summary that the lead replaces.',
  lead: 'Invented lead: a shared pool and three linked lamps decide the run.',
  phases: [
    { name: 'Lamp rooms', tag: 'two per room', items: [{ title: 'Split up', text: 'Two per room.', detail: 'BOT-ONLY wording about rooms.' }, 'A plain phase line.'] },
    // A repeating group of two toned phases, then a plain phase (timeline brackets, loop cue, tints).
    { name: 'Day', group: 'The keeper', cycle: true, tag: 'gauge fills', tone: 'yellow', items: [{ title: 'Bait the hop', text: 'Lead her to the colour you need.' }] },
    { name: 'Night', group: 'The keeper', cycle: true, tag: 'burst · no gauge', tone: 'blue', items: ['Invented night line.'] },
    { name: 'Last stand', items: ['Invented last line.'] },
  ],
  danger: [
    { title: 'Lamp flare', text: 'Clears the room after the last lamp.', detail: 'BOT-ONLY wording about flares.' },
    'A plain danger line.',
  ],
  tips: [{ title: 'One caller', text: 'Call the numbers aloud.' }],
  difficulty_notes: { h: { title: 'Hard twist', text: 'A random second room is hit.' } },
  notes: [{ title: 'Old build', text: 'No longer works after the room rule.' }, 'A plain note line.'],
  mechanics: [
    {
      kind: 'ledger',
      title: 'Pool · 1000 shared',
      rows: [
        { label: 'Death', value: '−100', direction: 'down' },
        { label: 'Clean phase', value: '+300', direction: 'up' },
        { label: 'Neutral row', value: '0' },
      ],
      note: 'Invented ledger note.',
    },
    {
      kind: 'zones',
      title: 'Room zones',
      zones: [
        { name: 'Left', sub: 'red', tone: 'red' },
        { name: 'Centre', sub: 'yellow', tone: 'yellow' },
        { name: 'Right', sub: 'green', tone: 'green' },
      ],
    },
    {
      kind: 'scale',
      title: 'Lamp scale',
      bands: [
        { label: '0', span: 6, tone: 'risk' },
        { label: 'low', span: 19, tone: 'neutral' },
        { label: '250 – 750 safe', span: 50, tone: 'safe' },
        { label: 'high', span: 19, tone: 'blue' },
        { label: '1000', span: 6, tone: 'risk' },
      ],
      note: 'Invented scale note.',
    },
  ],
  difficulties: [
    {
      name: 'Hard',
      entry_level: 275,
      boss_level: 285,
      pdr_percent: 380,
      party_max: 6,
      force: { kind: 'sacred', value: 350 },
      hp: [
        { phase: '1', value: '900t', count: 3, target: 'Lamps' },
        { phase: '2', value: '1.2q' },
        { phase: '3-1', value: '1.05q', count: 2, target: 'Lamps' },
        { phase: '3-2', value: '2.1q', target: 'Keeper' },
        { phase: 'total', value: '6q' },
      ],
      recommended_spec: { kind: 'HEXA stat', text: 'Invented long recommendation.', value: '≈ 99k', basis: 'Invented, 2026' },
      notes: ['Invented Hard note.'],
    },
    {
      name: 'Destiny',
      boss_level: 285,
      party_max: 1,
      force: { kind: 'sacred', value: 350 },
      hp: [{ phase: '1', value: '900t', count: 3 }, { phase: '2', value: '1.2q' }],
      mission: {
        series: 'destiny-weapon',
        order: 2,
        title: 'Invented mission title',
        modifier: { text: '+20% Final Damage', direction: 'up' },
        needs: '3,000 Invented Resolve',
        rules: ['Practice counts', 'No Cross World'],
      },
    },
  ],
  strategies: [
    { name: 'Balanced lamps', when: 'Parties and first clears.', risk: 'low', damage: 'high', payoff: 'Keeps the pool.', steps: ['Step one.', 'Step two.', 'Step three.'] },
    { name: 'Two-lamp break', when: 'Solo near the cut.', risk: 'high', damage: 'low', payoff: 'Near-full uptime.', steps: ['Only step.'] },
  ],
  sources: [
    { url: 'https://example.invalid/patch', title: 'Invented patch notes', author: 'Nobody', kind: 'official', fetched: '2026-10-05' },
    { url: 'https://example.invalid/guide', title: 'Invented guide', author: 'Someone', kind: 'guide', fetched: '2026-10-05', updated: '2026-10-01' },
    { url: 'https://example.invalid/wiki', title: 'Invented wiki', author: 'Wiki', kind: 'wiki', fetched: '2026-10-02' },
  ],
};

export const GUIDE_MISSIONS = [
  { series: 'destiny-weapon', order: 1, key: 'Seren', name: 'Invented First', difficulty: 'Destiny' },
  { series: 'destiny-weapon', order: 2, key: 'MaleficStar', name: 'Invented Second', difficulty: 'Destiny' },
  { series: 'destiny-weapon', order: 3, key: 'Kalos', name: 'Invented Third', difficulty: 'Destiny' },
];
