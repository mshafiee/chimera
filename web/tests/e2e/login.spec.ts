import { test, expect } from '@playwright/test'

test.describe('Login Flow', () => {
  test('login page loads and shows wallet connection prompt', async ({ page }) => {
    await page.goto('/login')
    await expect(page.getByText(/connect a wallet|authenticate/i)).toBeVisible()
  })

  test('redirects unauthenticated users to login', async ({ page }) => {
    await page.goto('/dashboard')
    await expect(page).toHaveURL(/\/login/)
  })
})
