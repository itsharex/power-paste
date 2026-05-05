import { getCurrentWindow } from '@tauri-apps/api/window'
import { watch, nextTick } from 'vue'

// 窗口尺寸配置
const WINDOW_SIZES = {
  settings: {
    width: 600,
    height: 500,
  },
}

// 保存用户在主面板的窗口尺寸
let savedHomeSize = null
let isResizing = false
let currentRouteName = null

/**
 * 窗口尺寸管理 composable
 * 根据路由自动调整窗口尺寸
 * - 切换到设置面板：自动调整为固定尺寸（600x500）
 * - 切换回主面板：恢复用户之前调整的尺寸
 */
export function useWindowSize(route) {
  const appWindow = getCurrentWindow()

  // 监听路由变化，自动调整窗口尺寸
  watch(
    () => route.name,
    async (routeName, oldRouteName) => {
      // 避免重复调整或相同路由
      if (isResizing || routeName === currentRouteName) {
        return
      }

      try {
        isResizing = true

        // 获取当前窗口尺寸
        const currentSize = await appWindow.innerSize()

        // 从主面板切换到设置面板
        if (oldRouteName === 'home' && routeName === 'settings') {
          // 保存主面板的当前尺寸
          savedHomeSize = {
            width: currentSize.width,
            height: currentSize.height,
          }

          // 切换到设置面板的固定尺寸
          const targetSize = WINDOW_SIZES.settings

          // 等待下一帧，确保 DOM 更新
          await nextTick()

          await appWindow.setSize({
            type: 'Logical',
            width: targetSize.width,
            height: targetSize.height,
          })

          currentRouteName = routeName
          await new Promise((resolve) => setTimeout(resolve, 150))
        }
        // 从设置面板切换回主面板
        else if (oldRouteName === 'settings' && routeName === 'home') {
          // 恢复主面板之前保存的尺寸
          if (savedHomeSize) {
            await nextTick()

            await appWindow.setSize({
              type: 'Logical',
              width: savedHomeSize.width,
              height: savedHomeSize.height,
            })

            currentRouteName = routeName
            await new Promise((resolve) => setTimeout(resolve, 150))
          } else {
            // 如果没有保存的尺寸（首次启动），不做调整
            currentRouteName = routeName
          }
        }
        // 其他路由切换（如主面板 <-> 互传面板）
        else {
          // 不做窗口尺寸调整，保持用户当前的窗口大小
          currentRouteName = routeName
        }
      } catch (error) {
        console.error('Failed to resize window:', error)
      } finally {
        isResizing = false
      }
    },
  )
}
