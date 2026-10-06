import { useState } from 'react';

import { Button } from '../../components/Button';
import { ExternalLink } from '../../components/ExternalLink';
import { Field, InputGroup } from '../../components/Field';
import { useSettingsFields } from './useSettingsFields';
import './CredentialFields.css';

/** OAuth client ID and secret, with setup instructions. Used in Settings and onboarding. */
export function CredentialFields() {
  const { text } = useSettingsFields();
  const [revealSecret, setRevealSecret] = useState(false);

  return (
    <>
      <Field label="OAuth client ID" htmlFor="f-client-id">
        <input
          id="f-client-id"
          type="text"
          spellCheck={false}
          autoComplete="off"
          placeholder="1234567890-abc….apps.googleusercontent.com"
          {...text('clientId')}
        />
      </Field>
      <Field label="OAuth client secret" htmlFor="f-client-secret">
        <InputGroup>
          <input
            id="f-client-secret"
            type={revealSecret ? 'text' : 'password'}
            spellCheck={false}
            autoComplete="off"
            placeholder="GOCSPX-…"
            {...text('clientSecret')}
          />
          <Button size="sm" onClick={() => setRevealSecret((r) => !r)}>
            {revealSecret ? 'Hide' : 'Show'}
          </Button>
        </InputGroup>
      </Field>
      <CredentialsHelp />
    </>
  );
}

function CredentialsHelp() {
  return (
    <details className="help">
      <summary>How do I get these?</summary>
      <ol>
        <li>
          Open the <ExternalLink href="https://console.cloud.google.com/projectcreate">Google Cloud console</ExternalLink>{' '}
          and create a project (any name).
        </li>
        <li>
          Go to <b>APIs &amp; Services → Library</b>, search for <b>Google Drive API</b> and click <b>Enable</b>.
        </li>
        <li>
          Open <b>OAuth consent screen</b> (Google Auth Platform). Choose user type <b>External</b>, fill in the app
          name and your email, and add yourself as a test user.
        </li>
        <li>
          Under <b>Audience</b>, click <b>Publish app</b> so the publishing status is <b>In production</b>. In
          “Testing” mode Google expires sign-ins after 7 days. (You can ignore the “unverified app” warning when you
          sign in — it is your own app.)
        </li>
        <li>
          Go to <b>Credentials → Create credentials → OAuth client ID</b>, choose application type <b>Desktop app</b>,
          and create it.
        </li>
        <li>
          Copy the <b>Client ID</b> and <b>Client secret</b> into the fields above.
        </li>
      </ol>
    </details>
  );
}
