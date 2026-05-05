# 窗口尺寸自动调整功能

## 功能说明

应用现在支持根据不同页面自动调整窗口尺寸，并带有平滑的过渡动画。

## 窗口尺寸配置

### 主面板（历史列表）
- 宽度：380px
- 高度：760px
- 瘦长布局，适合显示历史记录列表

### 设置面板
- 宽度：600px
- 高度：500px
- 宽矮布局，适合显示设置选项

### 互传面板
- 宽度：380px
- 高度：760px
- 与主面板相同的瘦长布局

## 技术实现

### 1. 窗口尺寸管理 Composable

创建了 `src/composables/useWindowSize.js`，负责：
- 监听路由变化
- 自动调整窗口尺寸
- 提供平滑的动画过渡效果

### 2. 动画效果

- **动画时长**：300ms
- **缓动函数**：easeInOutCubic（提供平滑的加速和减速）
- **视觉反馈**：窗口内容在调整时会有轻微的透明度变化

### 3. CSS 过渡

在 `src/styles/base.css` 中添加了：
```css
.window-shell {
    transition: opacity 200ms ease;
}

.window-shell.resizing {
    opacity: 0.92;
}
```

## 使用方式

功能已自动集成到应用中，无需手动配置。当用户：
1. 点击设置按钮 → 窗口自动变为宽矮布局（600x500）
2. 从设置返回主面板 → 窗口自动恢复瘦长布局（380x760）
3. 切换到互传面板 → 窗口保持瘦长布局（380x760）

## 自定义配置

如需调整窗口尺寸，可修改 `src/composables/useWindowSize.js` 中的 `WINDOW_SIZES` 配置：

```javascript
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
```

## 注意事项

1. 窗口尺寸调整使用 Tauri 的 `setSize` API
2. 动画通过 `requestAnimationFrame` 实现，确保流畅性
3. 如果窗口已经是目标尺寸，不会触发调整动画
4. 错误会被捕获并记录到控制台，不会影响应用正常使用
