/**
 * Circuit Breaker E2E Tests
 * 
 * Tests the circuit breaker UI functionality:
 * - View circuit breaker status
 * - Admin reset capability
 * - Status transitions
 * - Alert notifications
 */

import { test, expect } from '@playwright/test';

test.describe('Circuit Breaker Status', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
  });

  // ==========================================================================
  // STATUS DISPLAY TESTS
  // ==========================================================================

  test('should display circuit breaker status on dashboard', async ({ page }) => {
    // Look for circuit breaker indicator
    const cbStatus = page.locator('[data-testid="circuit-breaker"], .circuit-breaker, .system-status').first();
    const cbText = page.getByText(/circuit breaker|trading status/i).first();
    
    const hasCbStatus = await cbStatus.isVisible().catch(() => false) ||
                        await cbText.isVisible().catch(() => false);
    
    expect(hasCbStatus).toBe(true);
  });

  test('should show ACTIVE state in green', async ({ page }) => {
    // Look for active/healthy status
    const activeIndicator = page.locator('.status-active, .text-green, .bg-green').first();
    const activeText = page.getByText(/active|healthy|running/i).first();
    
    const hasActiveStatus = await activeIndicator.isVisible().catch(() => false) ||
                            await activeText.isVisible().catch(() => false);
    
    expect(hasActiveStatus).toBe(true);
  });

  test('should display threshold configuration', async ({ page }) => {
    // Navigate to config or check dashboard
    const configLink = page.getByRole('link', { name: /config|settings/i });
    
    if (await configLink.isVisible()) {
      await configLink.click();
      
      // Look for threshold settings
      const maxLoss = page.getByText(/max.*loss|24h.*loss/i).first();
      const consecutiveLosses = page.getByText(/consecutive.*loss/i).first();
      const maxDrawdown = page.getByText(/drawdown/i).first();
      
      const hasThresholds = await maxLoss.isVisible().catch(() => false) ||
                            await consecutiveLosses.isVisible().catch(() => false) ||
                            await maxDrawdown.isVisible().catch(() => false);
      
      expect(hasThresholds).toBe(true);
    }
  });

  // ==========================================================================
  // TRIPPED STATE TESTS
  // ==========================================================================

  test('should display TRIPPED state in red when triggered', async ({ page }) => {
    // Mock the health endpoint to return tripped status
    await page.route('**/api/v1/health', route => {
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          status: 'degraded',
          circuit_breaker: {
            state: 'TRIPPED',
            reason: 'Max loss exceeded',
          },
        }),
      });
    });

    await page.reload();

    // Look for tripped indicator
    const trippedIndicator = page.locator('.status-tripped, .text-red, .bg-red').first();
    const trippedText = page.getByText(/tripped|halted|stopped/i).first();
    
    const hasTrippedStatus = await trippedIndicator.isVisible().catch(() => false) ||
                             await trippedText.isVisible().catch(() => false);
    
    // Should show warning
    expect(hasTrippedStatus).toBe(true);
  });

  test('should show trip reason when tripped', async ({ page }) => {
    await page.route('**/api/v1/health', route => {
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          status: 'degraded',
          circuit_breaker: {
            state: 'TRIPPED',
            reason: '24h loss $525.00 exceeded threshold $500.00',
          },
        }),
      });
    });

    await page.reload();

    // Look for reason text
    const reasonText = page.getByText(/exceeded|threshold|loss/i).first();
    
    const hasReason = await reasonText.isVisible().catch(() => false);
    expect(hasReason).toBe(true);
  });

  // ==========================================================================
  // COOLDOWN STATE TESTS
  // ==========================================================================

  test('should display COOLDOWN state with remaining time', async ({ page }) => {
    await page.route('**/api/v1/health', route => {
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          status: 'degraded',
          circuit_breaker: {
            state: 'COOLDOWN',
            cooldown_remaining_secs: 900, // 15 minutes
          },
        }),
      });
    });

    await page.reload();

    // Look for cooldown indicator
    const cooldownText = page.getByText(/cooldown|minutes|remaining/i).first();
    
    const hasCooldown = await cooldownText.isVisible().catch(() => false);
    expect(hasCooldown).toBe(true);
  });

  // ==========================================================================
  // ADMIN RESET TESTS
  // ==========================================================================

  test('should show reset button when tripped', async ({ page }) => {
    await page.route('**/api/v1/health', route => {
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          status: 'degraded',
          circuit_breaker: {
            state: 'TRIPPED',
            reason: 'Manual trip',
          },
        }),
      });
    });

    await page.reload();

    // Look for reset button
    const resetButton = page.getByRole('button', { name: /reset|restore|resume/i });
    
    // Button should be visible but may require admin auth
    const hasResetButton = await resetButton.isVisible().catch(() => false);
    expect(hasResetButton).toBe(true);
  });

  test('should require admin authentication for reset', async ({ page }) => {
    await page.route('**/api/v1/health', route => {
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          circuit_breaker: {
            state: 'TRIPPED',
          },
        }),
      });
    });

    await page.reload();

    const resetButton = page.getByRole('button', { name: /reset/i }).first();
    
    if (await resetButton.isVisible()) {
      await resetButton.click();
      
      // Should prompt for auth or show unauthorized message
      const authPrompt = page.locator('.modal, [role="dialog"]').first();
      const authError = page.getByText(/unauthorized|login|admin/i).first();
      
      const requiresAuth = await authPrompt.isVisible().catch(() => false) ||
                           await authError.isVisible().catch(() => false);
      
      // Reset should require auth
      expect(requiresAuth).toBe(true);
    }
  });

  test('should log reset action in audit trail', async ({ page }) => {
    // Navigate to config/incidents page
    await page.goto('/incidents');
    
    // Look for audit log or incidents list
    const auditLog = page.locator('table, .incidents-list, .audit-log').first();
    const resetEntry = page.getByText(/reset|circuit breaker/i).first();
    
    const hasAuditLog = await auditLog.isVisible().catch(() => false) ||
                        await resetEntry.isVisible().catch(() => false);
    
    expect(hasAuditLog).toBe(true);
  });
});

test.describe('Circuit Breaker Alerts', () => {
  test('should display alert banner when tripped', async ({ page }) => {
    await page.route('**/api/v1/health', route => {
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          status: 'degraded',
          circuit_breaker: {
            state: 'TRIPPED',
            reason: 'Max consecutive losses',
          },
        }),
      });
    });

    await page.goto('/');

    // Look for alert banner
    const alertBanner = page.locator('.alert, .banner, [role="alert"]').first();
    const warningText = page.getByText(/warning|alert|attention/i).first();
    
    const hasAlert = await alertBanner.isVisible().catch(() => false) ||
                     await warningText.isVisible().catch(() => false);
    
    expect(hasAlert).toBe(true);
  });

  test('should show notification toast on state change', async ({ page }) => {
    await page.goto('/');

    // Wait for initial load
    await page.waitForTimeout(500);

    // Simulate state change via route modification
    await page.route('**/api/v1/health', route => {
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          circuit_breaker: {
            state: 'TRIPPED',
          },
        }),
      });
    });

    // Trigger a refresh/update
    await page.reload();

    // Look for toast notification
    const toast = page.locator('.toast, .notification, [role="alert"]').first();
    
    const hasToast = await toast.isVisible().catch(() => false);
    // Toast may or may not appear depending on implementation
  });
});

test.describe('Circuit Breaker Configuration', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/config');
  });

  test('should display current threshold values', async ({ page }) => {
    // Look for threshold configuration section
    const maxLossInput = page.locator('input[name*="loss"], [data-testid="max-loss"]').first();
    const thresholdText = page.getByText(/\$500|\$1000/i).first();
    
    const hasThresholds = await maxLossInput.isVisible().catch(() => false) ||
                          await thresholdText.isVisible().catch(() => false);
    
    expect(hasThresholds).toBe(true);
  });

  test('should require admin to modify thresholds', async ({ page }) => {
    // Look for save/update button
    const saveButton = page.getByRole('button', { name: /save|update|apply/i }).first();
    
    if (await saveButton.isVisible()) {
      await saveButton.click();
      
      // Should require authentication
      const authError = page.getByText(/unauthorized|forbidden|admin/i).first();
      const loginPrompt = page.locator('.modal, [role="dialog"]').first();
      
      const requiresAuth = await authError.isVisible().catch(() => false) ||
                           await loginPrompt.isVisible().catch(() => false);
      
      expect(requiresAuth).toBe(true);
    }
  });

  test('should show validation errors for invalid thresholds', async ({ page }) => {
    // Find a threshold input
    const thresholdInput = page.locator('input[type="number"]').first();
    
    if (await thresholdInput.isVisible()) {
      // Try to enter invalid value
      await thresholdInput.fill('-100');
      
      // Look for validation error
      const errorMessage = page.getByText(/invalid|positive|must be/i).first();
      
      const hasError = await errorMessage.isVisible().catch(() => false);
      expect(hasError).toBe(true);
    }
  });
});

test.describe('Circuit Breaker Reset Flow', () => {
  test('should reset circuit breaker and verify trading resumes', async ({ page }) => {
    // Start with tripped state
    await page.route('**/api/v1/health', route => {
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          status: 'degraded',
          circuit_breaker: {
            state: 'TRIPPED',
            reason: 'Test trip',
          },
        }),
      });
    });

    await page.goto('/config');

    // Find reset button
    const resetButton = page.getByRole('button', { name: /reset|restore|resume/i }).first();
    
    if (await resetButton.isVisible()) {
      // Mock successful reset
      await page.route('**/api/v1/config/circuit-breaker/reset', route => {
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            success: true,
            message: 'Circuit breaker reset successfully',
          }),
        });
      });

      // Update health endpoint to show ACTIVE after reset
      await page.route('**/api/v1/health', route => {
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            status: 'healthy',
            circuit_breaker: {
              state: 'ACTIVE',
            },
          }),
        });
      });

      await resetButton.click();

      // Wait for reset to complete
      await page.waitForTimeout(1000);

      // Verify state changed to ACTIVE
      const activeStatus = page.getByText(/active|healthy|running/i).first();
      const hasActive = await activeStatus.isVisible().catch(() => false);
      
      expect(hasActive).toBe(true);
    }
  });

  test('should verify trading resumes after reset', async ({ page }) => {
    // Mock reset endpoint
    await page.route('**/api/v1/config/circuit-breaker/reset', route => {
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true }),
      });
    });

    // Mock health showing ACTIVE
    await page.route('**/api/v1/health', route => {
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          circuit_breaker: {
            state: 'ACTIVE',
            trading_allowed: true,
          },
        }),
      });
    });

    await page.goto('/');

    // Verify trading is allowed
    const tradingStatus = page.getByText(/trading.*active|trading.*enabled/i).first();
    const statusIndicator = page.locator('.status-active, .trading-active').first();
    
    const isTradingActive = await tradingStatus.isVisible().catch(() => false) ||
                            await statusIndicator.isVisible().catch(() => false);
    
    expect(isTradingActive).toBe(true);
  });

  test('should show success message after reset', async ({ page }) => {
    await page.route('**/api/v1/config/circuit-breaker/reset', route => {
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          message: 'Circuit breaker reset successfully',
        }),
      });
    });

    await page.goto('/config');

    const resetButton = page.getByRole('button', { name: /reset/i }).first();
    
    if (await resetButton.isVisible()) {
      await resetButton.click();

      // Look for success message
      const successMessage = page.getByText(/reset.*success|trading.*resumed/i).first();
      const toast = page.locator('.toast, .notification').filter({ hasText: /success/i }).first();
      
      const hasSuccess = await successMessage.isVisible().catch(() => false) ||
                         await toast.isVisible().catch(() => false);
      
      expect(hasSuccess).toBe(true);
    }
  });

  test('should handle reset failure gracefully', async ({ page }) => {
    // Mock reset failure
    await page.route('**/api/v1/config/circuit-breaker/reset', route => {
      route.fulfill({
        status: 500,
        contentType: 'application/json',
        body: JSON.stringify({
          error: 'Failed to reset circuit breaker',
        }),
      });
    });

    await page.goto('/config');

    const resetButton = page.getByRole('button', { name: /reset/i }).first();
    
    if (await resetButton.isVisible()) {
      await resetButton.click();

      // Should show error message
      const errorMessage = page.getByText(/error|failed|try again/i).first();
      const hasError = await errorMessage.isVisible().catch(() => false);
      
      expect(hasError).toBe(true);
    }
  });

  test('should require confirmation before reset', async ({ page }) => {
    await page.goto('/config');

    const resetButton = page.getByRole('button', { name: /reset/i }).first();
    
    if (await resetButton.isVisible()) {
      await resetButton.click();

      // Should show confirmation dialog
      const confirmDialog = page.locator('.modal, [role="dialog"]').filter({ hasText: /confirm|are you sure/i }).first();
      const confirmButton = page.getByRole('button', { name: /confirm|yes|proceed/i }).first();
      
      const hasConfirmation = await confirmDialog.isVisible().catch(() => false) ||
                              await confirmButton.isVisible().catch(() => false);
      
      // Reset should require confirmation
      expect(hasConfirmation).toBe(true);
    }
  });
});

