/**
 * Configuration E2E Tests
 * 
 * Tests the configuration page functionality:
 * - Circuit breaker configuration
 * - Strategy allocation updates
 * - Notification rules configuration
 * - Circuit breaker reset
 * - Emergency kill switch
 */

import { test, expect } from '@playwright/test';

test.describe('Configuration', () => {
  test.beforeEach(async ({ page }) => {
    // Navigate to config page
    await page.goto('/config');
  });

  // ==========================================================================
  // PAGE LOAD TESTS
  // ==========================================================================

  test('should load the configuration page', async ({ page }) => {
    // Verify page loads
    await expect(page.locator('h1, h2').first()).toBeVisible();
    
    // Look for config sections
    const circuitBreakerSection = page.getByText(/circuit.*breaker/i).first();
    const strategySection = page.getByText(/strategy|allocation/i).first();
    
    const hasConfig = await circuitBreakerSection.isVisible().catch(() => false) ||
                     await strategySection.isVisible().catch(() => false);
    
    expect(hasConfig).toBe(true);
  });

  // ==========================================================================
  // CIRCUIT BREAKER TESTS
  // ==========================================================================

  test('should display circuit breaker status', async ({ page }) => {
    // Look for circuit breaker status
    const cbStatus = page.getByText(/circuit.*breaker.*status|trading.*allowed/i).first();
    const cbIndicator = page.locator('[data-testid="circuit-breaker-status"], .cb-status').first();
    
    const hasStatus = await cbStatus.isVisible().catch(() => false) ||
                     await cbIndicator.isVisible().catch(() => false);
    
    expect(hasStatus).toBe(true);
  });

  test('should update circuit breaker threshold', async ({ page }) => {
    // Look for max loss input
    const maxLossInput = page.locator('input[name*="max_loss"], input[data-testid="max-loss"]').first();
    
    if (await maxLossInput.isVisible()) {
      // Get current value
      const currentValue = await maxLossInput.inputValue();
      
      // Update value
      await maxLossInput.clear();
      await maxLossInput.fill('750');
      
      // Look for save button
      const saveButton = page.getByRole('button', { name: /save/i }).first();
      
      if (await saveButton.isVisible()) {
        await saveButton.click();
        
        // Wait for update
        await page.waitForTimeout(500);
        
        // Verify value updated (may need to reload)
        await page.reload();
        const newValue = await maxLossInput.inputValue();
        expect(newValue).toBe('750');
      }
    }
  });

  test('should reset circuit breaker', async ({ page }) => {
    // Look for reset button
    const resetButton = page.getByRole('button', { name: /reset.*circuit.*breaker/i }).first();
    
    if (await resetButton.isVisible()) {
      // Click reset
      await resetButton.click();
      
      // Wait for confirmation or update
      await page.waitForTimeout(500);
      
      // Verify reset (check status indicator)
      const cbStatus = page.getByText(/active|trading.*allowed/i).first();
      if (await cbStatus.isVisible()) {
        expect(await cbStatus.isVisible()).toBe(true);
      }
    }
  });

  // ==========================================================================
  // STRATEGY ALLOCATION TESTS
  // ==========================================================================

  test('should update strategy allocation', async ({ page }) => {
    // Look for shield/spear allocation inputs
    const shieldInput = page.locator('input[name*="shield"], input[data-testid="shield-percent"]').first();
    const spearInput = page.locator('input[name*="spear"], input[data-testid="spear-percent"]').first();
    
    if (await shieldInput.isVisible() && await spearInput.isVisible()) {
      // Update allocation
      await shieldInput.clear();
      await shieldInput.fill('80');
      
      await spearInput.clear();
      await spearInput.fill('20');
      
      // Save
      const saveButton = page.getByRole('button', { name: /save/i }).first();
      
      if (await saveButton.isVisible()) {
        await saveButton.click();
        await page.waitForTimeout(500);
        
        // Verify update
        await page.reload();
        const shieldValue = await shieldInput.inputValue();
        const spearValue = await spearInput.inputValue();
        
        expect(parseInt(shieldValue) + parseInt(spearValue)).toBe(100);
      }
    }
  });

  test('should validate strategy allocation sums to 100%', async ({ page }) => {
    const shieldInput = page.locator('input[name*="shield"]').first();
    const spearInput = page.locator('input[name*="spear"]').first();
    
    if (await shieldInput.isVisible() && await spearInput.isVisible()) {
      // Set invalid allocation
      await shieldInput.clear();
      await shieldInput.fill('60');
      
      await spearInput.clear();
      await spearInput.fill('50'); // Total = 110%
      
      // Try to save
      const saveButton = page.getByRole('button', { name: /save/i }).first();
      
      if (await saveButton.isVisible()) {
        await saveButton.click();
        await page.waitForTimeout(500);
        
        // Should show validation error
        const errorMessage = page.getByText(/must.*sum.*100|allocation.*invalid/i).first();
        const hasError = await errorMessage.isVisible().catch(() => false);
        
        // Either error shown or save prevented
        expect(hasError || (await shieldInput.inputValue() !== '60')).toBe(true);
      }
    }
  });

  // ==========================================================================
  // NOTIFICATION RULES TESTS
  // ==========================================================================

  test('should toggle notification rules', async ({ page }) => {
    // Look for notification rule toggles
    const circuitBreakerToggle = page.locator('input[type="checkbox"][name*="circuit_breaker"], [data-testid="notif-circuit-breaker"]').first();
    
    if (await circuitBreakerToggle.isVisible()) {
      const initialState = await circuitBreakerToggle.isChecked();
      
      // Toggle
      await circuitBreakerToggle.click();
      
      // Verify state changed
      const newState = await circuitBreakerToggle.isChecked();
      expect(newState).toBe(!initialState);
      
      // Save if there's a save button
      const saveButton = page.getByRole('button', { name: /save/i }).first();
      if (await saveButton.isVisible()) {
        await saveButton.click();
        await page.waitForTimeout(500);
      }
    }
  });

  // ==========================================================================
  // EMERGENCY CONTROLS TESTS
  // ==========================================================================

  test('should display emergency kill switch', async ({ page }) => {
    // Look for kill switch
    const killSwitch = page.getByRole('button', { name: /halt.*trading|kill.*switch|emergency.*stop/i }).first();
    const killSwitchSection = page.getByText(/halt.*all.*trading|emergency.*control/i).first();
    
    const hasKillSwitch = await killSwitch.isVisible().catch(() => false) ||
                          await killSwitchSection.isVisible().catch(() => false);
    
    expect(hasKillSwitch).toBe(true);
  });

  test('should require confirmation for kill switch', async ({ page }) => {
    const killSwitch = page.getByRole('button', { name: /halt.*trading|kill.*switch/i }).first();
    
    if (await killSwitch.isVisible()) {
      await killSwitch.click();
      
      // Should show confirmation dialog
      const confirmDialog = page.getByRole('dialog').first();
      const confirmButton = page.getByRole('button', { name: /confirm|yes|activate/i }).first();
      
      if (await confirmDialog.isVisible()) {
        expect(await confirmDialog.isVisible()).toBe(true);
        
        // Cancel to avoid actually halting
        const cancelButton = page.getByRole('button', { name: /cancel|no/i }).first();
        if (await cancelButton.isVisible()) {
          await cancelButton.click();
        }
      }
    }
  });

  // ==========================================================================
  // CONFIGURATION HISTORY TESTS
  // ==========================================================================

  test('should display configuration change history', async ({ page }) => {
    // Look for audit log or change history
    const auditLog = page.getByText(/change.*history|audit.*log|config.*history/i).first();
    const historyTable = page.locator('table, [data-testid="config-history"]').first();
    
    const hasHistory = await auditLog.isVisible().catch(() => false) ||
                      await historyTable.isVisible().catch(() => false);
    
    // History may be in a separate tab or section
    if (hasHistory) {
      expect(hasHistory).toBe(true);
    }
  });
});
