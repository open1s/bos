export class DomainError extends Error {
  constructor(message: string, public readonly code: string) {
    super(message);
    this.name = 'DomainError';
  }
}

export class ContradictionNotFoundError extends DomainError {
  constructor(id: string) {
    super(`Contradiction with id '${id}' not found`, 'CONTRADICTION_NOT_FOUND');
  }
}

export class InvalidParameterError extends DomainError {
  constructor(parameter: string) {
    super(`Invalid TRIZ parameter: ${parameter}`, 'INVALID_PARAMETER');
  }
}

export class PrincipleNotFoundError extends DomainError {
  constructor(index: number) {
    super(`Inventive principle with index '${index}' not found`, 'PRINCIPLE_NOT_FOUND');
  }
}
