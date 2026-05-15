import { SearchResult } from '../../domain/solution/search_port.js';
import { ReferenceSourceType } from '../../domain/solution/external_reference.js';

const MOCK_PATENTS: Record<string, SearchResult[]> = {
  default: [
    {
      title: 'US20230012345A1 - Hybrid Powertrain with Variable Displacement Engine',
      url: 'https://patents.google.com/patent/US20230012345A1',
      snippet: 'A hybrid powertrain system that dynamically adjusts engine displacement based on speed requirements, reducing fuel consumption at high speeds while maintaining acceleration performance.',
      sourceType: 'patent' as ReferenceSourceType,
      publishedDate: '2023-03-15',
      authors: ['Zhang Wei', 'Li Ming'],
    },
    {
      title: 'US20220098765A1 - Aerodynamic Vehicle Body with Active Drag Reduction',
      url: 'https://patents.google.com/patent/US20220098765A1',
      snippet: 'An active aerodynamic system that reduces drag coefficient at high speeds by adjusting body panels, improving fuel efficiency without sacrificing top speed.',
      sourceType: 'patent' as ReferenceSourceType,
      publishedDate: '2022-11-20',
      authors: ['Tanaka Hiroshi'],
    },
    {
      title: 'US20240056789A1 - Regenerative Braking with Kinetic Energy Recovery for High-Speed Vehicles',
      url: 'https://patents.google.com/patent/US20240056789A1',
      snippet: 'A regenerative braking system optimized for high-speed driving conditions, recovering kinetic energy during deceleration to offset increased fuel consumption.',
      sourceType: 'patent' as ReferenceSourceType,
      publishedDate: '2024-01-10',
      authors: ['Mueller Hans', 'Schmidt Anna'],
    },
  ],
};

const MOCK_PAPERS: Record<string, SearchResult[]> = {
  default: [
    {
      title: 'Optimizing Vehicle Speed-Energy Trade-offs Using TRIZ Contradiction Resolution',
      url: 'https://www.semanticscholar.org/paper/triz-vehicle-optimization',
      snippet: 'This paper applies TRIZ inventive principles to resolve the speed vs. fuel consumption contradiction in automotive design. Principles 15 (Dynamics), 28 (Mechanics Substitution), and 35 (Parameter Changes) are identified as most effective.',
      sourceType: 'paper' as ReferenceSourceType,
      publishedDate: '2023',
      authors: ['Chen L.', 'Wang Y.', 'Kumar R.'],
    },
    {
      title: 'Application of TRIZ Principle 28 (Mechanics Substitution) in Hybrid Vehicle Design',
      url: 'https://www.semanticscholar.org/paper/triz-hybrid-mechanics',
      snippet: 'Demonstrates how replacing purely mechanical powertrains with electro-mechanical hybrid systems resolves the speed-energy contradiction. Case studies show 15-25% fuel savings at highway speeds.',
      sourceType: 'paper' as ReferenceSourceType,
      publishedDate: '2022',
      authors: ['Park S.', 'Kim J.'],
    },
    {
      title: 'Dynamic Parameter Adjustment in Automotive Systems: A TRIZ Approach',
      url: 'https://www.semanticscholar.org/paper/dynamic-triz-automotive',
      snippet: 'Explores how TRIZ Principle 15 (Dynamics) can be applied to create adaptive vehicle systems that optimize the speed-fuel trade-off in real-time based on driving conditions.',
      sourceType: 'paper' as ReferenceSourceType,
      publishedDate: '2024',
      authors: ['Ivanov A.', 'Petrov D.'],
    },
  ],
};

const MOCK_TECH: Record<string, SearchResult[]> = {
  default: [
    {
      title: 'How Tesla Resolves the Speed vs. Range Contradiction',
      url: 'https://electrek.co/tesla-speed-range-optimization',
      snippet: 'Tesla\'s approach to the speed-energy contradiction uses multiple TRIZ principles: segmentation (modular battery packs), dynamics (adaptive power distribution), and mechanics substitution (regenerative braking).',
      sourceType: 'tech_solution' as ReferenceSourceType,
      publishedDate: '2024-02-15',
    },
    {
      title: 'Toyota\'s Hybrid Synergy Drive: TRIZ in Practice',
      url: 'https://www.toyota.com/hybrid-technology',
      snippet: 'Toyota\'s Hybrid Synergy Drive exemplifies TRIZ Principle 6 (Universality) - the electric motor serves multiple functions: acceleration assist, regenerative braking, and idle stop-start.',
      sourceType: 'tech_solution' as ReferenceSourceType,
      publishedDate: '2023-08-20',
    },
  ],
};

export function getMockPatents(query: string, maxResults = 5): SearchResult[] {
  return MOCK_PATENTS.default.slice(0, maxResults);
}

export function getMockPapers(query: string, maxResults = 5): SearchResult[] {
  return MOCK_PAPERS.default.slice(0, maxResults);
}

export function getMockTechSolutions(query: string, maxResults = 5): SearchResult[] {
  return MOCK_TECH.default.slice(0, maxResults);
}

export function hasMockData(): boolean {
  return true;
}
