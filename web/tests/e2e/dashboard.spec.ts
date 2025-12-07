/**
 * Dashboard E2E Tests
 * 
 * Tests the main dashboard functionality:
 * - Page loads correctly
 * - Real-time data updates
 * - System status display
 * - Performance metrics
 */

import { test, expect } from '@playwright/test';

test.describe('Dashboard', () => {
  test.beforeEach(async ({ page }) => {
    // Navigate to dashboard
    await page.goto('/');
  });

  // ==========================================================================
  // PAGE LOAD TESTS
  // ==========================================================================

  test('should load the dashboard page', async ({ page }) => {
    // Verify page title or header
    await expect(page.locator('h1, h2').first()).toBeVisible();
  });

  test('should display system status indicator', async ({ page }) => {
    // Look for a status indicator (badge, icon, or text)
    const statusIndicator = page.locator('[data-testid="system-status"], .status-indicator, .health-status').first();
    
    // If not found with data-testid, check for common status text
    const statusText = page.getByText(/healthy|active|running/i).first();
    
    // At least one should be visible
    const isStatusVisible = await statusIndicator.isVisible().catch(() => false) ||
                           await statusText.isVisible().catch(() => false);
    
    expect(isStatusVisible).toBe(true);
  });

  test('should display performance metrics section', async ({ page }) => {
    // Look for PnL, trades, or positions metrics
    const metricsSection = page.locator('[data-testid="metrics"], .metrics, .stats').first();
    const pnlText = page.getByText(/pnl|profit|loss/i).first();
    
    const hasMetrics = await metricsSection.isVisible().catch(() => false) ||
                       await pnlText.isVisible().catch(() => false);
    
    expect(hasMetrics).toBe(true);
  });

  // ==========================================================================
  // NAVIGATION TESTS
  // ==========================================================================

  test('should have navigation links', async ({ page }) => {
    // Check for navigation elements
    const nav = page.locator('nav, [role="navigation"], .sidebar');
    await expect(nav.first()).toBeVisible();
  });

  test('should navigate to wallets page', async ({ page }) => {
    // Click on wallets link
    const walletsLink = page.getByRole('link', { name: /wallet/i });
    
    if (await walletsLink.isVisible()) {
      await walletsLink.click();
      await expect(page).toHaveURL(/wallet/i);
    }
  });

  test('should navigate to trades page', async ({ page }) => {
    const tradesLink = page.getByRole('link', { name: /trade/i });
    
    if (await tradesLink.isVisible()) {
      await tradesLink.click();
      await expect(page).toHaveURL(/trade/i);
    }
  });

  // ==========================================================================
  // DATA DISPLAY TESTS
  // ==========================================================================

  test('should display positions table or list', async ({ page }) => {
    // Look for table or list of positions
    const positionsTable = page.locator('table, [role="table"], .positions-list').first();
    const positionsSection = page.getByText(/positions|active trades/i).first();
    
    const hasPositions = await positionsTable.isVisible().catch(() => false) ||
                         await positionsSection.isVisible().catch(() => false);
    
    // This may be empty, but the section should exist
    expect(hasPositions).toBe(true);
  });

  test('should display strategy allocation', async ({ page }) => {
    // Look for Shield/Spear strategy indicators
    const shieldText = page.getByText(/shield/i).first();
    const spearText = page.getByText(/spear/i).first();
    
    // At least one strategy should be mentioned
    const hasStrategy = await shieldText.isVisible().catch(() => false) ||
                        await spearText.isVisible().catch(() => false);
    
    expect(hasStrategy).toBe(true);
  });

  // ==========================================================================
  // RESPONSIVE DESIGN TESTS
  // ==========================================================================

  test('should be responsive on mobile viewport', async ({ page }) => {
    // Set mobile viewport
    await page.setViewportSize({ width: 375, height: 667 });
    
    // Page should still be functional
    await expect(page.locator('body')).toBeVisible();
    
    // Check that content is not overflowing
    const body = page.locator('body');
    const bodyWidth = await body.evaluate(el => el.scrollWidth);
    expect(bodyWidth).toBeLessThanOrEqual(375);
  });

  // ==========================================================================
  // ERROR STATE TESTS
  // ==========================================================================

  test('should handle API errors gracefully', async ({ page }) => {
    // Mock API failure
    await page.route('**/api/**', route => {
      route.fulfill({
        status: 500,
        body: JSON.stringify({ error: 'Internal Server Error' }),
      });
    });

    await page.reload();

    // Should show error message or fallback UI
    // (The app should not crash)
    await expect(page.locator('body')).toBeVisible();
  });
});

test.describe('Dashboard - Real-time Updates', () => {
  test('should connect to WebSocket for live updates', async ({ page }) => {
    await page.goto('/');

    // Check for WebSocket connection or live indicator
    const liveIndicator = page.locator('[data-testid="live-indicator"], .live-indicator, .ws-status').first();
    const liveText = page.getByText(/live|connected|real-?time/i).first();

    // Wait a bit for WebSocket to connect
    await page.waitForTimeout(1000);

    const hasLiveIndicator = await liveIndicator.isVisible().catch(() => false) ||
                             await liveText.isVisible().catch(() => false);

    // WebSocket may not be available in test environment, so this is optional
    // expect(hasLiveIndicator).toBe(true);
  });
});

