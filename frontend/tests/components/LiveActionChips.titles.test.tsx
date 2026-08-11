import { describe, expect, test } from 'bun:test';
import { render, screen } from '@testing-library/react';
import { LiveActionChips } from '../../src/app/_components/LiveActionChips';
import type { LiveActionChipState } from '../../src/hooks/useLiveActionChips';

const BASE_STATE: LiveActionChipState = {
  result: '',
  isLoading: false,
  error: null,
  isRetryable: false,
  hasGenerated: false,
};

describe('LiveActionChips trigger button titles (glass restyle regression)', () => {
  test('the Recap and Questions to ask trigger buttons keep their title attributes after the restyle', () => {
    render(
      <LiveActionChips
        chips={{ recap: BASE_STATE, questions: BASE_STATE }}
        generate={() => {}}
        hasActivity
        isRecording
      />
    );

    expect(screen.getByTitle('Recap')).not.toBeNull();
    expect(screen.getByTitle('Questions to ask')).not.toBeNull();
  });
});
