export interface InventivePrinciple {
  index: number;
  name: string;
  description: string;
  examples: string[];
}

export const INVENTIVE_PRINCIPLES: ReadonlyArray<InventivePrinciple> = [
  { index: 1, name: 'Segmentation', description: 'Divide an object into independent parts', examples: ['Modular furniture', 'Assembly line production'] },
  { index: 2, name: 'Taking out', description: 'Separate an interfering part or isolate the necessary part', examples: ['Noise barriers', 'Remote sensors'] },
  { index: 3, name: 'Local quality', description: 'Change an object\'s structure from uniform to non-uniform', examples: ['Ergonomic tools', 'Gradient materials'] },
  { index: 4, name: 'Asymmetry', description: 'Change the shape of an object from symmetrical to asymmetrical', examples: ['Asymmetric tires', 'One-way valves'] },
  { index: 5, name: 'Merging', description: 'Bring closer together identical or similar objects', examples: ['Multi-core processors', 'Parallel computing'] },
  { index: 6, name: 'Universality', description: 'Make a part or object perform multiple functions', examples: ['Swiss Army knife', 'Smartphone'] },
  { index: 7, name: 'Nested doll', description: 'Place one object inside another', examples: ['Telescopic antenna', 'Russian dolls'] },
  { index: 8, name: 'Anti-weight', description: 'Compensate for the weight of an object by merging with another', examples: ['Helium balloons', 'Counterweights'] },
  { index: 9, name: 'Preliminary anti-action', description: 'If it is necessary to perform an action with both harmful and useful effects, use anti-action', examples: ['Pre-stressed concrete', 'Buffer solutions'] },
  { index: 10, name: 'Preliminary action', description: 'Perform the required change before it is needed', examples: ['Pre-cut materials', 'Pre-positioned tools'] },
  { index: 11, name: 'Beforehand cushioning', description: 'Compensate for relatively low reliability by arranging countermeasures beforehand', examples: ['Safety belts', 'Backup systems'] },
  { index: 12, name: 'Equipotentiality', description: 'Change the working conditions to eliminate the need for raising or lowering', examples: ['Assembly line height adjustment', 'Loading docks'] },
  { index: 13, name: 'The other way around', description: 'Invert the action(s) used to solve the problem', examples: ['Cooling by heating', 'Moving treadmill instead of runner'] },
  { index: 14, name: 'Spheroidality', description: 'Change from linear parts to curved surfaces', examples: ['Ball bearings', 'Dome structures'] },
  { index: 15, name: 'Dynamics', description: 'Allow characteristics of an object or environment to change to be optimal', examples: ['Adjustable steering wheel', 'Flexible hoses'] },
  { index: 16, name: 'Partial or excessive actions', description: 'If 100% of an effect is hard to achieve, use slightly less or more', examples: ['Overfilling then trimming', 'Partial coating'] },
  { index: 17, name: 'Another dimension', description: 'Move an object in two or three dimensional space', examples: ['Multi-story buildings', '3D printing'] },
  { index: 18, name: 'Mechanical vibration', description: 'Cause an object to oscillate or vibrate', examples: ['Ultrasonic cleaning', 'Vibratory feeders'] },
  { index: 19, name: 'Periodic action', description: 'Replace continuous action with periodic or pulsating', examples: ['Pulsed lasers', 'Intermittent wipers'] },
  { index: 20, name: 'Continuity of useful action', description: 'Carry on work continuously; avoid idle or intermediate actions', examples: ['Continuous production', 'Flywheel energy storage'] },
  { index: 21, name: 'Skipping', description: 'Conduct a process or certain stages at high speed', examples: ['Quick-change tooling', 'Rapid prototyping'] },
  { index: 22, name: 'Blessing in disguise', description: 'Turn harmful factors into secondary benefits', examples: ['Waste heat recovery', 'Byproduct utilization'] },
  { index: 23, name: 'Feedback', description: 'Introduce feedback to improve a process or action', examples: ['Thermostat control', 'Quality control loops'] },
  { index: 24, name: 'Intermediary', description: 'Use an intermediary carrier article or intermediary process', examples: ['Mediator in negotiations', 'Carrier proteins'] },
  { index: 25, name: 'Self-service', description: 'Make an object serve itself by performing auxiliary functions', examples: ['Self-cleaning ovens', 'Self-healing materials'] },
  { index: 26, name: 'Copying', description: 'Use a simplified and inexpensive copy instead of an unavailable object', examples: ['Virtual reality', 'Photocopying'] },
  { index: 27, name: 'Cheap short-living objects', description: 'Replace an inexpensive object with multiple inexpensive ones', examples: ['Disposable cameras', 'Single-use medical tools'] },
  { index: 28, name: 'Mechanics substitution', description: 'Replace a mechanical system with a non-mechanical one', examples: ['Touch screens', 'Magnetic levitation'] },
  { index: 29, name: 'Pneumatics and hydraulics', description: 'Use gas or liquid parts instead of solid parts', examples: ['Hydraulic brakes', 'Pneumatic tools'] },
  { index: 30, name: 'Flexible shells and thin films', description: 'Use flexible shells and thin films instead of three-dimensional structures', examples: ['Bubble wrap', 'Greenhouse films'] },
  { index: 31, name: 'Porous materials', description: 'Make an object porous or add porous elements', examples: ['Filter membranes', 'Breathable fabrics'] },
  { index: 32, name: 'Color changes', description: 'Change the color of an object or its external environment', examples: ['Thermal imaging', 'pH indicators'] },
  { index: 33, name: 'Homogeneity', description: 'Make objects interacting with a given object of the same material', examples: ['Diamond cutting diamond', 'Same-metal welding'] },
  { index: 34, name: 'Discarding and recovering', description: 'Make portions of an object that have fulfilled their functions go away', examples: ['Dissolving capsules', 'Biodegradable packaging'] },
  { index: 35, name: 'Parameter changes', description: 'Change the physical state or concentration of an object', examples: ['Phase change materials', 'Concentrated solutions'] },
  { index: 36, name: 'Phase transition', description: 'Use phenomena occurring during phase transitions', examples: ['Heat pipes', 'Shape memory alloys'] },
  { index: 37, name: 'Thermal expansion', description: 'Use thermal expansion or contraction of materials', examples: ['Bimetallic strips', 'Shrink fitting'] },
  { index: 38, name: 'Strong oxidants', description: 'Replace common air with oxygen-enriched air', examples: ['Oxygen welding', 'Ozone treatment'] },
  { index: 39, name: 'Inert atmosphere', description: 'Replace a normal environment with an inert one', examples: ['Argon welding', 'Nitrogen packaging'] },
  { index: 40, name: 'Composite materials', description: 'Change from uniform materials to composite materials', examples: ['Carbon fiber', 'Fiberglass'] },
];

export function getPrincipleByIndex(index: number): InventivePrinciple | undefined {
  return INVENTIVE_PRINCIPLES.find(p => p.index === index);
}

export function getPrincipleByName(name: string): InventivePrinciple | undefined {
  return INVENTIVE_PRINCIPLES.find(p => p.name.toLowerCase() === name.toLowerCase());
}
