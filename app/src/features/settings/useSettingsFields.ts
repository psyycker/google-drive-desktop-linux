import type { ChangeEvent } from 'react';

import { useSettings } from '../../state/SettingsContext';
import type { SettingsForm } from '../../state/settingsForm';

type KeysOf<T> = { [K in keyof SettingsForm]: SettingsForm[K] extends T ? K : never }[keyof SettingsForm];

/** Binds inputs to the settings form: `<input {...text('pollSecs')} />`. */
export function useSettingsFields() {
  const { form, edit } = useSettings();
  return {
    text: (key: KeysOf<string>) => ({
      value: form[key],
      onChange: (e: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) =>
        edit({ [key]: e.target.value } as Partial<SettingsForm>),
    }),
    flag: (key: KeysOf<boolean>) => ({
      checked: form[key],
      onChange: (checked: boolean) => edit({ [key]: checked } as Partial<SettingsForm>),
    }),
  };
}
