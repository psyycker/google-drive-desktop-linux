import { createContext, useContext } from 'react';

/**
 * A context that must be provided: returns its provider and a hook that throws when
 * used outside it, so consumers never deal with a missing value.
 */
export function createStrictContext<T>(name: string) {
  const Context = createContext<T | null>(null);
  Context.displayName = name;
  const useStrictContext = (): T => {
    const value = useContext(Context);
    if (value === null) throw new Error(`use${name}() must be used inside <${name}Provider>`);
    return value;
  };
  return [Context.Provider, useStrictContext] as const;
}
