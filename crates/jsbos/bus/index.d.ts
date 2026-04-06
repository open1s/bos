/**
 * TypeScript declarations for ESM wrapper of Bus
 */

export interface BusConfig {
  mode: string;
  connect?: string[];
  listen?: string[];
  peer?: string;
}

export interface Bus {
  sessionId(): string;
  publishText(topic: string, payload: string): Promise<void>;
  publishJson(topic: string, data: unknown): Promise<void>;
  createPublisher(topic: string): Promise<Publisher>;
  createSubscriber(topic: string): Promise<Subscriber>;
  createQuery(topic: string): Promise<Query>;
  createQueryable(topic: string): Promise<Queryable>;
  createCaller(name: string): Promise<Caller>;
  createCallable(uri: string): Promise<Callable>;
}

export type BusStatic = {
  create(config?: BusConfig): Promise<Bus>;
};

export interface Publisher {
  readonly topic: string;
  publishText(payload: string): Promise<void>;
  publishJson(data: unknown): Promise<void>;
}

export type PublisherStatic = {
  new(topic: string): Promise<Publisher>;
  withSession(topic: string, session: unknown): Promise<Publisher>;
};

export interface Subscriber {
  readonly topic: string;
  recv(): Promise<string | null>;
  recvWithTimeoutMs(timeoutMs: number): Promise<string | null>;
  recvJsonWithTimeoutMs(timeoutMs: number): Promise<unknown | null>;
  run(handler: (msg: unknown) => unknown): Promise<void>;
  runJson(handler: (msg: unknown) => unknown): Promise<void>;
  stop(): Promise<void>;
}

export type SubscriberStatic = {
  new(topic: string): Promise<Subscriber>;
  withSession(topic: string, session: unknown): Promise<Subscriber>;
};

export interface Query {
  readonly topic: string;
  queryText(payload: string): Promise<string>;
  queryTextTimeoutMs(payload: string, timeoutMs: number): Promise<string>;
}

export type QueryStatic = {
  new(topic: string): Promise<Query>;
  withSession(topic: string, session: unknown): Promise<Query>;
};

export interface Queryable {
  setHandler(handler: (input: string) => unknown): void;
  start(): Promise<void>;
  run(handler: (input: string) => unknown): Promise<void>;
  runJson(handler: (input: string) => unknown): Promise<void>;
  runStream(handler: (input: string) => unknown): Promise<void>;
}

export type QueryableStatic = {
  new(topic: string): Promise<Queryable>;
};

export interface Caller {
  callText(payload: string): Promise<string>;
}

export type CallerStatic = {
  new(name: string): Promise<Caller>;
  withSession(name: string, session: unknown): Promise<Caller>;
};

export interface Callable {
  setHandler(handler: (input: string) => unknown): void;
  readonly isStarted: boolean;
  start(): Promise<void>;
  run(handler: (input: string) => unknown): Promise<void>;
  runJson(handler: (input: string) => unknown): Promise<void>;
}

export type CallableStatic = {
  new(): Callable;
};

declare const Bus: BusStatic;
declare const Publisher: PublisherStatic;
declare const Subscriber: SubscriberStatic;
declare const Query: QueryStatic;
declare const Queryable: QueryableStatic;
declare const Caller: CallerStatic;
declare const Callable: CallableStatic;

export { Bus, Publisher, Subscriber, Query, Queryable, Caller, Callable };
