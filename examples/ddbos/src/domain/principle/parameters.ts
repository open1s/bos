export interface TrizParameter {
  index: number;
  name: string;
  description: string;
}

export const TRIZ_PARAMETERS: ReadonlyArray<TrizParameter> = [
  { index: 1, name: 'Weight of moving object', description: 'Weight of an object that moves within the system' },
  { index: 2, name: 'Weight of stationary object', description: 'Weight of an object that does not move' },
  { index: 3, name: 'Length of moving object', description: 'Length of any moving object' },
  { index: 4, name: 'Length of stationary object', description: 'Length of any stationary object' },
  { index: 5, name: 'Area of moving object', description: 'Area of any moving object' },
  { index: 6, name: 'Area of stationary object', description: 'Area of any stationary object' },
  { index: 7, name: 'Volume of moving object', description: 'Volume of any moving object' },
  { index: 8, name: 'Volume of stationary object', description: 'Volume of any stationary object' },
  { index: 9, name: 'Speed', description: 'Rate of motion or action' },
  { index: 10, name: 'Force', description: 'Intensity of interaction between objects' },
  { index: 11, name: 'Stress or pressure', description: 'Force per unit area' },
  { index: 12, name: 'Shape', description: 'Form or configuration of an object' },
  { index: 13, name: 'Stability', description: 'Composition and integrity of the system' },
  { index: 14, name: 'Strength', description: 'Ability to resist changes' },
  { index: 15, name: 'Durability of moving object', description: 'How long a moving object lasts' },
  { index: 16, name: 'Durability of stationary object', description: 'How long a stationary object lasts' },
  { index: 17, name: 'Temperature', description: 'Degree of heat in the system' },
  { index: 18, name: 'Brightness', description: 'Intensity of light or illumination' },
  { index: 19, name: 'Energy spent by moving object', description: 'Energy used by moving parts' },
  { index: 20, name: 'Energy spent by stationary object', description: 'Energy used by stationary parts' },
  { index: 21, name: 'Power', description: 'Rate of energy transfer' },
  { index: 22, name: 'Loss of energy', description: 'Wasted or dissipated energy' },
  { index: 23, name: 'Loss of substance', description: 'Wasted or lost material' },
  { index: 24, name: 'Loss of information', description: 'Lost or degraded data' },
  { index: 25, name: 'Loss of time', description: 'Wasted time or delays' },
  { index: 26, name: 'Amount of substance', description: 'Quantity of material' },
  { index: 27, name: 'Reliability', description: 'Consistency of performance' },
  { index: 28, name: 'Measurement accuracy', description: 'Precision of measurements' },
  { index: 29, name: 'Manufacturing precision', description: 'Precision of production' },
  { index: 30, name: 'External harm affects object', description: 'Harmful factors from outside' },
  { index: 31, name: 'Object-generated harmful factors', description: 'Harmful side effects produced' },
  { index: 32, name: 'Ease of manufacturing', description: 'How easy it is to make' },
  { index: 33, name: 'Ease of operation', description: 'How easy it is to use' },
  { index: 34, name: 'Ease of repair', description: 'How easy it is to fix' },
  { index: 35, name: 'Adaptability', description: 'Ability to adapt to conditions' },
  { index: 36, name: 'Device complexity', description: 'Complexity of the system' },
  { index: 37, name: 'Difficulty of detection', description: 'How hard it is to detect' },
  { index: 38, name: 'Degree of automation', description: 'Level of automatic operation' },
  { index: 39, name: 'Productivity', description: 'Output per unit of input' },
];

export function getParameterByIndex(index: number): TrizParameter | undefined {
  return TRIZ_PARAMETERS.find(p => p.index === index);
}

export function getParameterByName(name: string): TrizParameter | undefined {
  return TRIZ_PARAMETERS.find(p => p.name.toLowerCase() === name.toLowerCase());
}
