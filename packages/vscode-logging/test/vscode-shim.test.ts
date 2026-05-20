import { describe, expect, it, vi } from 'vitest';
import { type Disposable, type DisposableStore, EventEmitter } from '../src/vscode-shim.js';

/**
 * Exercises vscode shim behavior in the test area through focused scenarios.
 * The cases lock in edge handling and regression coverage so refactors preserve expected state
 * transitions and output.
 *
 * @summary Tests vscode shim behavior in test
 */

describe('EventEmitter', () => {
  it('should fire events to listeners', () => {
    const emitter = new EventEmitter<string>();
    const listener = vi.fn();

    emitter.event(listener);
    emitter.fire('test');

    expect(listener).toHaveBeenCalledWith('test');
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it('should support multiple listeners', () => {
    const emitter = new EventEmitter<number>();
    const listener1 = vi.fn();
    const listener2 = vi.fn();

    emitter.event(listener1);
    emitter.event(listener2);
    emitter.fire(42);

    expect(listener1).toHaveBeenCalledWith(42);
    expect(listener2).toHaveBeenCalledWith(42);
  });

  it('should bind thisArgs when provided', () => {
    const emitter = new EventEmitter<string>();
    const context = { value: 'context' };
    let capturedThis: unknown;

    const listener = function (this: unknown) {
      capturedThis = this;
    };

    emitter.event(listener, context);
    emitter.fire('test');

    expect(capturedThis).toBe(context);
  });

  it('should add disposable to array when provided', () => {
    const emitter = new EventEmitter<string>();
    const disposables: Disposable[] = [];
    const listener = vi.fn();

    emitter.event(listener, undefined, disposables);

    expect(disposables).toHaveLength(1);
    expect(disposables[0]).toHaveProperty('dispose');
  });

  it('should add disposable to DisposableStore when provided', () => {
    const emitter = new EventEmitter<string>();
    const addSpy = vi.fn();
    const store: DisposableStore = { add: addSpy };
    const listener = vi.fn();

    const disposable = emitter.event(listener, undefined, store);

    expect(addSpy).toHaveBeenCalledWith(disposable);
  });

  it('should remove listener when disposable is disposed', () => {
    const emitter = new EventEmitter<string>();
    const listener = vi.fn();

    const disposable = emitter.event(listener);
    emitter.fire('before');
    expect(listener).toHaveBeenCalledTimes(1);

    disposable.dispose();
    emitter.fire('after');
    expect(listener).toHaveBeenCalledTimes(1); // Still 1, not called again
  });

  it('should catch errors in listeners and continue firing to other listeners', () => {
    const emitter = new EventEmitter<string>();
    const errorListener = vi.fn(() => {
      throw new Error('listener error');
    });
    const successListener = vi.fn();
    const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    emitter.event(errorListener);
    emitter.event(successListener);
    emitter.fire('test');

    expect(errorListener).toHaveBeenCalledWith('test');
    expect(successListener).toHaveBeenCalledWith('test');
    expect(consoleErrorSpy).toHaveBeenCalled();

    consoleErrorSpy.mockRestore();
  });

  it('should handle listener modifications during fire', () => {
    const emitter = new EventEmitter<string>();
    const listener1 = vi.fn();
    const listener2 = vi.fn(() => {
      // Try to add a listener during fire
      emitter.event(vi.fn());
    });
    const listener3 = vi.fn();

    emitter.event(listener1);
    emitter.event(listener2);
    emitter.event(listener3);

    emitter.fire('test');

    // All original listeners should be called
    expect(listener1).toHaveBeenCalledWith('test');
    expect(listener2).toHaveBeenCalledWith('test');
    expect(listener3).toHaveBeenCalledWith('test');
  });

  it('should report hasListeners correctly', () => {
    const emitter = new EventEmitter<string>();
    const listener = vi.fn();

    expect(emitter.hasListeners()).toBe(false);

    const disposable = emitter.event(listener);
    expect(emitter.hasListeners()).toBe(true);

    disposable.dispose();
    expect(emitter.hasListeners()).toBe(false);
  });

  it('should not fire after dispose', () => {
    const emitter = new EventEmitter<string>();
    const listener = vi.fn();

    emitter.event(listener);
    emitter.dispose();
    emitter.fire('test');

    expect(listener).not.toHaveBeenCalled();
  });

  it('should return no-op disposable when subscribing after dispose', () => {
    const emitter = new EventEmitter<string>();
    const listener = vi.fn();

    emitter.dispose();
    const disposable = emitter.event(listener);

    emitter.fire('test');
    expect(listener).not.toHaveBeenCalled();

    // Disposing the no-op disposable should not throw
    expect(() => disposable.dispose()).not.toThrow();
  });

  it('should clear all listeners on dispose', () => {
    const emitter = new EventEmitter<string>();
    const listener1 = vi.fn();
    const listener2 = vi.fn();

    emitter.event(listener1);
    emitter.event(listener2);

    expect(emitter.hasListeners()).toBe(true);
    emitter.dispose();
    expect(emitter.hasListeners()).toBe(false);
  });
});
