import { useCallback, useMemo, useRef, useState, type ReactNode } from 'react';

import { Toast } from '../components/Toast';
import { errorMessage } from '../lib/format';
import { createStrictContext } from './strictContext';

export interface ToastApi {
  /** Shows a short confirmation. */
  info(text: string): void;
  /** Shows an error: a message, a thrown `Error` or a rejected command. */
  error(e: unknown): void;
  /** Awaits `task` and toasts `success`; on failure toasts the error and rethrows it. */
  run<T>(task: Promise<T>, success?: string): Promise<T>;
  /** Like `run`, for calls nothing waits on: failures are only toasted. */
  fire(task: Promise<unknown>, success?: string): void;
}

interface Message {
  id: number;
  text: string;
  error: boolean;
}

const [Provider, useToast] = createStrictContext<ToastApi>('Toast');
export { useToast };

export function ToastProvider({ children }: { children: ReactNode }) {
  const [message, setMessage] = useState<Message | null>(null);
  const nextId = useRef(0);
  const timer = useRef<number | undefined>(undefined);

  const show = useCallback((text: string, error: boolean) => {
    window.clearTimeout(timer.current);
    setMessage({ id: ++nextId.current, text, error });
    timer.current = window.setTimeout(() => setMessage(null), error ? 6000 : 3000);
  }, []);

  const api = useMemo<ToastApi>(() => {
    const run = async <T,>(task: Promise<T>, success?: string) => {
      try {
        const result = await task;
        if (success) show(success, false);
        return result;
      } catch (e) {
        show(errorMessage(e), true);
        throw e;
      }
    };
    return {
      info: (text) => show(text, false),
      error: (e) => show(errorMessage(e), true),
      run,
      fire: (task, success) => {
        run(task, success).catch(() => {});
      },
    };
  }, [show]);

  return (
    <Provider value={api}>
      {children}
      {message && (
        <Toast key={message.id} error={message.error}>
          {message.text}
        </Toast>
      )}
    </Provider>
  );
}
