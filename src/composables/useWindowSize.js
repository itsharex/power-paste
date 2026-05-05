import { getCurrentWindow } from '@tauri-apps/api/window'
import { watch } from 'vue'

// 窗口尺寸配置
const WINDOW_SIZES = {
  home: {
    width: 380,
    height: 760,
  },
  settings: {
    width: 600,
    height: 500,
  },
  lanTransfer: {
    width: 380,
    height: 760,
  },
}

/**
 * 窗口尺寸管理 composable
 * 根据路由自动调整窗口尺寸，并添加过渡动画
 */
export function useWindowSize(route) {
  const appWindow = getCurrentWindow()

  // 监听路由变化，自动调整窗口尺寸
  watch(
    () => route.name,
    async (routeName) => {
      const targetSize = WINDOW_SIZES[routeName] || WINDOW_SIZES.home

      try {
        // 获取当前窗口尺寸
        const currentSize = await appWindow.innerSize()

        // 如果尺寸已经匹配，不需要调整
        if (
          currentSize.width === targetSize.width &&
          currentSize.height === targetSize.height
        ) {
          return
        }

        // 添加 resizing 类以触发 CSS 过渡
        const windowShell = document.querySelector('.window-shell')
        if (windowShell) {
          windowShell.classList.add('resizing')
        }

        // 使用动画过渡调整窗口尺寸
        await animateWindowResize(
          appWindow,
          currentSize,
          targetSize,
          300, // 动画持续时间 300ms
        )

        // 移除 resizing 类
        if (windowShell) {
          setTimeout(() => {
            windowShell.classList.remove('resizing')
          }, 100)
        }
      } catch (error) {
        console.error('Failed to resize window:', error)
      }
    },
    { immediate: true },
  )
}

/**
 * 窗口尺寸动画过渡
 * @param {Window} appWindow - Tauri 窗口实例
 * @param {Object} from - 起始尺寸 { width, height }
 * @param {Object} to - 目标尺寸 { width, height }
 * @param {number} duration - 动画持续时间（毫秒）
 */
async function animateWindowResize(appWindow, from, to, duration) {
  const startTime = Date.now()
  const startWidth = from.width
  const startHeight = from.height
  const deltaWidth = to.width - startWidth
  const deltaHeight = to.height - startHeight

  return new Promise((resolve) => {
    const animate = async () => {
      const elapsed = Date.now() - startTime
      const progress = Math.min(elapsed / duration, 1)

      // 使用 easeInOutCubic 缓动函数
      const eased = easeInOutCubic(progress)

      const currentWidth = Math.round(startWidth + deltaWidth * eased)
      const currentHeight = Math.round(startHeight + deltaHeight * eased)

      try {
        await appWindow.setSize({
          type: 'Logical',
          width: currentWidth,
          height: currentHeight,
        })
      } catch (error) {
        console.error('Failed to set window size:', error)
      }

      if (progress < 1) {
        requestAnimationFrame(animate)
      } else {
        resolve()
      }
    }

    requestAnimationFrame(animate)
  })
}

/**
 * easeInOutCubic 缓动函数
 * 提供平滑的加速和减速效果
 */
function easeInOutCubic(t) {
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2
}
