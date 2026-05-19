import type { ReactNode } from 'react'
import { DialogFooter } from './dialog'

/**
 * Standard dialog footer with flex-row layout (mobile-friendly).
 * Drop-in for <DialogFooter className="flex-row gap-2 sm:space-x-0">.
 */
export function DialogActions({ children }: { children: ReactNode }) {
  return (
    <DialogFooter className="flex-row gap-2 sm:space-x-0">
      {children}
    </DialogFooter>
  )
}
