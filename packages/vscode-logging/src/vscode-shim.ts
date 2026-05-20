/**
 * Test-friendly VSCode API implementations.
 * Based on the tree package's superior EventEmitter implementation.
 *
 * @summary Test-friendly VSCode API implementations
 */

export interface Disposable {
  dispose(): void;
}

export interface DisposableStore {
  add<T extends Disposable>(disposable: T): T;
}

export type Event<T> = (
  listener: (e: T) => unknown,
  thisArgs?: unknown,
  disposables?: Disposable[] | DisposableStore
) => Disposable;

export class EventEmitter<T> implements Disposable {
  private listeners: Array<(e: T) => unknown> = [];
  private _disposed = false;

  readonly event: Event<T> = (
    listener: (e: T) => unknown,
    thisArgs?: unknown,
    disposables?: Disposable[] | DisposableStore
  ): Disposable => {
    if (this._disposed) {
      // Return a no-op disposable if already disposed
      return { dispose: () => {} };
    }

    // Bind thisArgs if provided
    const boundListener = thisArgs ? listener.bind(thisArgs) : listener;
    this.listeners.push(boundListener);

    const result: Disposable = {
      dispose: () => {
        const index = this.listeners.indexOf(boundListener);
        if (index !== -1) {
          this.listeners.splice(index, 1);
        }
      }
    };

    // Add to disposables if provided
    if (disposables) {
      if (Array.isArray(disposables)) {
        disposables.push(result);
      } else {
        disposables.add(result);
      }
    }

    return result;
  };

  fire(data: T): void {
    if (this._disposed) {
      return;
    }
    // Copy listeners array to handle modifications during iteration
    const listeners = this.listeners.slice();
    for (const listener of listeners) {
      try {
        listener(data);
      } catch (e) {
        console.error('Error in event listener:', e);
      }
    }
  }

  hasListeners(): boolean {
    return this.listeners.length > 0;
  }

  dispose(): void {
    this._disposed = true;
    this.listeners = [];
  }
}

// LogLevel enum
export const LogLevel = {
  Off: 0,
  Trace: 1,
  Debug: 2,
  Info: 3,
  Warning: 4,
  Error: 5
} as const;

export type LogLevel = (typeof LogLevel)[keyof typeof LogLevel];

// ViewColumn enum
export const ViewColumn = {
  Active: -1,
  Beside: -2,
  One: 1,
  Two: 2,
  Three: 3,
  Four: 4,
  Five: 5,
  Six: 6,
  Seven: 7,
  Eight: 8,
  Nine: 9
} as const;

export type ViewColumn = (typeof ViewColumn)[keyof typeof ViewColumn];

// OutputChannel interface
export interface OutputChannel {
  readonly name: string;
  append(value: string): void;
  appendLine(value: string): void;
  replace(value: string): void;
  clear(): void;
  show(preserveFocus?: boolean): void;
  show(column?: ViewColumn, preserveFocus?: boolean): void;
  hide(): void;
  dispose(): void;
}

// LogOutputChannel interface
export interface LogOutputChannel extends OutputChannel {
  readonly logLevel: LogLevel;
  readonly onDidChangeLogLevel: Event<LogLevel>;
  trace(message: string, ...args: unknown[]): void;
  debug(message: string, ...args: unknown[]): void;
  info(message: string, ...args: unknown[]): void;
  warn(message: string, ...args: unknown[]): void;
  error(error: string | Error, ...args: unknown[]): void;
}

// Stub implementations (exported for test use)
export class StubLogOutputChannel implements LogOutputChannel {
  readonly logLevel: LogLevel = LogLevel.Info;
  readonly onDidChangeLogLevel: Event<LogLevel>;

  constructor(readonly name: string) {
    this.onDidChangeLogLevel = new EventEmitter<LogLevel>().event;
  }

  append(_value: string): void {}
  appendLine(_value: string): void {}
  replace(_value: string): void {}
  clear(): void {}
  show(_columnOrPreserveFocus?: ViewColumn | boolean, _preserveFocus?: boolean): void {}
  hide(): void {}
  dispose(): void {}

  trace(_message: string, ..._args: unknown[]): void {}
  debug(_message: string, ..._args: unknown[]): void {}
  info(_message: string, ..._args: unknown[]): void {}
  warn(_message: string, ..._args: unknown[]): void {}
  error(_error: string | Error, ..._args: unknown[]): void {}
}

export class StubOutputChannel implements OutputChannel {
  constructor(readonly name: string) {}

  append(_value: string): void {}
  appendLine(_value: string): void {}
  replace(_value: string): void {}
  clear(): void {}
  show(_columnOrPreserveFocus?: ViewColumn | boolean, _preserveFocus?: boolean): void {}
  hide(): void {}
  dispose(): void {}
}

// window namespace
export namespace window {
  export function createOutputChannel(name: string, languageId?: string): OutputChannel;
  export function createOutputChannel(name: string, options: { log: true }): LogOutputChannel;
  /**
   * Creates a stubbed output channel for tests.
   *
   * @param name - Channel name shown in diagnostics and assertions
   * @param languageIdOrOptions - Language id for text channels or `{ log: true }` for log channels
   * @returns Stub output channel matching the requested VSCode overload
   */
  export function createOutputChannel(
    name: string,
    languageIdOrOptions?: string | { log: true }
  ): OutputChannel | LogOutputChannel {
    if (typeof languageIdOrOptions === 'object' && languageIdOrOptions.log === true) {
      return new StubLogOutputChannel(name);
    }
    return new StubOutputChannel(name);
  }
}
