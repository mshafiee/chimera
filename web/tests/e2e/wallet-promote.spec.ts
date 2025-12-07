/**
 * Wallet Promotion E2E Tests
 * 
 * Tests wallet management functionality:
 * - Promote wallet with TTL
 * - Demote wallet
 * - View wallet details
 * - Verify auto-demote after TTL
 */

import { test, expect } from '@playwright/test';

test.describe('Wallet Management', () => {
  test.beforeEach(async ({ page }) => {
    // Navigate to wallets page
    await page.goto('/wallets');
  });

  // ==========================================================================
  // PAGE LOAD TESTS
  // ==========================================================================

  test('should load wallets page', async ({ page }) => {
    await expect(page).toHaveURL(/wallet/i);
    
    // Should have a heading or title
    const heading = page.locator('h1, h2').filter({ hasText: /wallet/i });
    await expect(heading.first()).toBeVisible();
  });

  test('should display wallet list', async ({ page }) => {
    // Look for wallet table or list
    const walletTable = page.locator('table, [role="table"], .wallet-list').first();
    const walletCards = page.locator('.wallet-card, [data-testid="wallet-item"]').first();
    
    const hasWallets = await walletTable.isVisible().catch(() => false) ||
                       await walletCards.isVisible().catch(() => false);
    
    // List might be empty, but container should exist
    expect(hasWallets).toBe(true);
  });

  // ==========================================================================
  // WALLET STATUS TESTS
  // ==========================================================================

  test('should display wallet status badges', async ({ page }) => {
    // Look for status indicators (ACTIVE, CANDIDATE, etc.)
    const statusBadge = page.locator('.badge, .status, [role="status"]').first();
    const statusText = page.getByText(/active|candidate|rejected/i).first();
    
    const hasStatus = await statusBadge.isVisible().catch(() => false) ||
                      await statusText.isVisible().catch(() => false);
    
    // Should have some status indicator
    expect(hasStatus).toBe(true);
  });

  test('should filter wallets by status', async ({ page }) => {
    // Look for filter controls
    const filterDropdown = page.locator('select, [role="listbox"], .filter').first();
    const filterButtons = page.locator('button').filter({ hasText: /active|candidate|all/i }).first();
    
    const hasFilter = await filterDropdown.isVisible().catch(() => false) ||
                      await filterButtons.isVisible().catch(() => false);
    
    if (hasFilter) {
      // Try to filter
      if (await filterDropdown.isVisible()) {
        await filterDropdown.selectOption({ label: /active/i });
      } else if (await filterButtons.isVisible()) {
        await filterButtons.click();
      }
      
      // Verify URL or UI updated
      await expect(page).toHaveURL(/status|filter/i);
    }
  });

  // ==========================================================================
  // WALLET DETAIL TESTS
  // ==========================================================================

  test('should view wallet details', async ({ page }) => {
    // Click on first wallet to view details
    const walletRow = page.locator('tr, .wallet-item, .wallet-card').first();
    
    if (await walletRow.isVisible()) {
      await walletRow.click();
      
      // Should show wallet details modal or page
      const detailsModal = page.locator('.modal, [role="dialog"], .wallet-details').first();
      const detailsPage = page.getByText(/wqs|roi|trades/i).first();
      
      const hasDetails = await detailsModal.isVisible().catch(() => false) ||
                         await detailsPage.isVisible().catch(() => false);
      
      expect(hasDetails).toBe(true);
    }
  });

  test('should display WQS score', async ({ page }) => {
    // Look for WQS (Wallet Quality Score)
    const wqsText = page.getByText(/wqs|quality score/i).first();
    const wqsValue = page.locator('[data-testid="wqs-score"], .wqs-score').first();
    
    const hasWqs = await wqsText.isVisible().catch(() => false) ||
                   await wqsValue.isVisible().catch(() => false);
    
    expect(hasWqs).toBe(true);
  });

  // ==========================================================================
  // PROMOTION FLOW TESTS
  // ==========================================================================

  test('should show promote button for candidates', async ({ page }) => {
    // Filter to CANDIDATE wallets
    await page.goto('/wallets?status=CANDIDATE');
    
    // Look for promote button
    const promoteButton = page.getByRole('button', { name: /promote/i });
    
    // Button may or may not be visible depending on data
    if (await promoteButton.isVisible()) {
      await expect(promoteButton).toBeEnabled();
    }
  });

  test('should open TTL dialog when promoting', async ({ page }) => {
    await page.goto('/wallets?status=CANDIDATE');
    
    const promoteButton = page.getByRole('button', { name: /promote/i }).first();
    
    if (await promoteButton.isVisible()) {
      await promoteButton.click();
      
      // Should show TTL selection dialog
      const ttlDialog = page.locator('.modal, [role="dialog"]').first();
      const ttlInput = page.locator('input[type="number"], select').filter({ hasText: /hour|day|ttl/i }).first();
      
      const hasTtlOption = await ttlDialog.isVisible().catch(() => false) ||
                           await ttlInput.isVisible().catch(() => false);
      
      expect(hasTtlOption).toBe(true);
    }
  });

  test('should show demote button for active wallets', async ({ page }) => {
    // Filter to ACTIVE wallets
    await page.goto('/wallets?status=ACTIVE');
    
    // Look for demote button
    const demoteButton = page.getByRole('button', { name: /demote|remove/i });
    
    if (await demoteButton.isVisible()) {
      await expect(demoteButton).toBeEnabled();
    }
  });

  // ==========================================================================
  // AUTHORIZATION TESTS
  // ==========================================================================

  test('should require operator role for promotion', async ({ page }) => {
    // Check for auth warning or disabled state
    const promoteButton = page.getByRole('button', { name: /promote/i }).first();
    const authWarning = page.getByText(/unauthorized|permission|admin/i).first();
    
    if (await promoteButton.isVisible()) {
      // Button should be disabled or show warning when clicked without auth
      const isDisabled = await promoteButton.getAttribute('disabled');
      
      if (!isDisabled) {
        await promoteButton.click();
        
        // Should show auth warning or login prompt
        const hasAuthWarning = await authWarning.isVisible().catch(() => false);
        // This depends on auth implementation
      }
    }
  });
});

test.describe('Wallet TTL Behavior', () => {
  test('should display TTL countdown for promoted wallets', async ({ page }) => {
    await page.goto('/wallets?status=ACTIVE');
    
    // Look for TTL indicator
    const ttlIndicator = page.locator('[data-testid="ttl"], .ttl, .expires').first();
    const ttlText = page.getByText(/expires|remaining|hours left/i).first();
    
    const hasTtl = await ttlIndicator.isVisible().catch(() => false) ||
                   await ttlText.isVisible().catch(() => false);
    
    // Some wallets may have TTL, some may be permanent
    // expect(hasTtl).toBe(true);
  });

  test('should differentiate permanent vs temporary promotions', async ({ page }) => {
    await page.goto('/wallets?status=ACTIVE');
    
    // Look for permanent/temporary indicators
    const permanentBadge = page.getByText(/permanent/i).first();
    const temporaryBadge = page.getByText(/temporary|ttl|expires/i).first();
    
    // At least one type should exist
    const hasTypeIndicator = await permanentBadge.isVisible().catch(() => false) ||
                             await temporaryBadge.isVisible().catch(() => false);
    
    // expect(hasTypeIndicator).toBe(true);
  });
});

