'use client';

import { useEffect } from 'react';

/**
 * Binds Cmd/Ctrl+J to the ask sidebar toggle, matching the hint printed in the
 * sidebar's footer. Shared by the live and meeting-details screens so the
 * shortcut can't drift between the two places the sidebar appears.
 */
export function useAskPanelShortcut(toggle: () => void) {
  useEffect(() => {
    const handleShortcut = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() === 'j' && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        toggle();
      }
    };
    window.addEventListener('keydown', handleShortcut);
    return () => window.removeEventListener('keydown', handleShortcut);
  }, [toggle]);
}
