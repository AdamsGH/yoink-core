/**
 * BlurDialog - a Dialog with a frosted-glass overlay.
 *
 * Drop-in for shadcn Dialog when you want backdrop-blur instead of a plain
 * dark overlay. The content panel is centred, max-w-lg on desktop, full-width
 * on mobile, and scrollable when tall.
 *
 * Usage:
 *   <BlurDialog open={open} onOpenChange={setOpen}>
 *     <BlurDialogContent className="max-w-xl">
 *       <BlurDialogHeader title="Title" description="optional" />
 *       {children}
 *       <BlurDialogFooter>...</BlurDialogFooter>
 *     </BlurDialogContent>
 *   </BlurDialog>
 */

import * as React from 'react'
import * as DialogPrimitive from '@radix-ui/react-dialog'
import { X } from 'lucide-react'
import { cn } from '@core/lib/utils'

// Re-export primitives that do not need customisation.
export const BlurDialog = DialogPrimitive.Root
export const BlurDialogTrigger = DialogPrimitive.Trigger
export const BlurDialogClose = DialogPrimitive.Close

// Overlay: dark + blur
const BlurDialogOverlay = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Overlay>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Overlay>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Overlay
    ref={ref}
    className={cn(
      'fixed inset-0 z-50 bg-black/50 backdrop-blur-sm',
      'data-[state=open]:animate-in data-[state=closed]:animate-out',
      'data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0',
      className,
    )}
    {...props}
  />
))
BlurDialogOverlay.displayName = 'BlurDialogOverlay'

// Content panel
export const BlurDialogContent = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Content>
>(({ className, children, ...props }, ref) => (
  <DialogPrimitive.Portal>
    <BlurDialogOverlay />
    <DialogPrimitive.Content
      ref={ref}
      className={cn(
        // Positioning
        'fixed left-1/2 top-1/2 z-50 -translate-x-1/2 -translate-y-1/2',
        // Size: full-width on mobile, capped on desktop
        'w-[calc(100vw-2rem)] max-w-lg',
        // Layout
        'flex flex-col max-h-[90dvh] overflow-hidden',
        // Appearance
        'rounded-xl border bg-background shadow-2xl',
        // Animations
        'duration-200',
        'data-[state=open]:animate-in data-[state=closed]:animate-out',
        'data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0',
        'data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95',
        'data-[state=closed]:slide-out-to-top-[2%] data-[state=open]:slide-in-from-top-[2%]',
        className,
      )}
      {...props}
    >
      {children}
      <DialogPrimitive.Close className="absolute right-3 top-3 rounded-md p-1 opacity-60 ring-offset-background transition-opacity hover:opacity-100 focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2">
        <X className="h-4 w-4" />
        <span className="sr-only">Close</span>
      </DialogPrimitive.Close>
    </DialogPrimitive.Content>
  </DialogPrimitive.Portal>
))
BlurDialogContent.displayName = 'BlurDialogContent'

// Header slot (sticky at top)
export function BlurDialogHeader({
  title,
  description,
  className,
}: {
  title: React.ReactNode
  description?: React.ReactNode
  className?: string
}) {
  return (
    <div className={cn('shrink-0 px-5 pt-5 pb-3 border-b border-border/60', className)}>
      <DialogPrimitive.Title className="text-base font-semibold leading-snug pr-6">
        {title}
      </DialogPrimitive.Title>
      {description && (
        <DialogPrimitive.Description asChild>
          <div className="mt-0.5">{description}</div>
        </DialogPrimitive.Description>
      )}
    </div>
  )
}

// Scrollable body
export function BlurDialogBody({
  children,
  className,
}: {
  children: React.ReactNode
  className?: string
}) {
  return (
    <div className={cn('flex-1 overflow-y-auto px-5 py-4', className)}>
      {children}
    </div>
  )
}

// Footer slot (sticky at bottom)
export function BlurDialogFooter({
  children,
  className,
}: {
  children: React.ReactNode
  className?: string
}) {
  return (
    <div className={cn('shrink-0 px-5 py-3 border-t border-border/60 flex items-center gap-2', className)}>
      {children}
    </div>
  )
}
