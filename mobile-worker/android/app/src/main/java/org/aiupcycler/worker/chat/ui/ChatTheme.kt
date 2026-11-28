package org.aiupcycler.worker.chat.ui

import android.provider.Settings
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.colorResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import org.aiupcycler.worker.R

/** Fira families are bundled in `res/font`, never fetched at runtime. */
val FiraSans = FontFamily(Font(R.font.fira_sans_regular))
val FiraSansMedium = FontFamily(Font(R.font.fira_sans_medium))
val FiraCode = FontFamily(Font(R.font.fira_code_regular))

/**
 * Device-level motion preference, derived from the Android animator duration
 * scale. Chat screens stay static, but later tasks can gate transitions on it.
 */
val LocalChatReducedMotion = compositionLocalOf { false }

/**
 * Dark-first chat theme. Every colour comes from `res/values/colors.xml`, so
 * the Compose surface and the existing XML theming cannot drift apart.
 */
@Composable
fun ChatTheme(
    darkTheme: Boolean = true,
    content: @Composable () -> Unit,
) {
    val context = LocalContext.current
    val reducedMotion = remember(context) {
        runCatching {
            Settings.Global.getFloat(
                context.contentResolver,
                Settings.Global.ANIMATOR_DURATION_SCALE,
                1f,
            ) == 0f
        }.getOrDefault(false)
    }

    val background = colorResource(R.color.aiu_background)
    val card = colorResource(R.color.aiu_card)
    val border = colorResource(R.color.aiu_border)
    val mutedText = colorResource(R.color.aiu_text_muted)
    val accent = colorResource(R.color.aiu_accent)
    val destructive = colorResource(R.color.aiu_destructive)

    val colorScheme = if (darkTheme) {
        darkColorScheme(
            primary = accent,
            onPrimary = background,
            secondary = accent,
            onSecondary = background,
            background = background,
            onBackground = mutedText,
            surface = card,
            onSurface = mutedText,
            surfaceVariant = card,
            onSurfaceVariant = mutedText,
            outline = border,
            outlineVariant = border,
            error = destructive,
            onError = background,
        )
    } else {
        lightColorScheme(
            primary = accent,
            onPrimary = background,
            secondary = accent,
            onSecondary = background,
            background = background,
            onBackground = mutedText,
            surface = card,
            onSurface = mutedText,
            surfaceVariant = card,
            onSurfaceVariant = mutedText,
            outline = border,
            outlineVariant = border,
            error = destructive,
            onError = background,
        )
    }

    val typography = Typography(
        titleLarge = TextStyle(fontFamily = FiraSansMedium, fontWeight = FontWeight.Medium),
        titleMedium = TextStyle(fontFamily = FiraSansMedium, fontWeight = FontWeight.Medium),
        bodyLarge = TextStyle(fontFamily = FiraSans),
        bodyMedium = TextStyle(fontFamily = FiraSans),
        bodySmall = TextStyle(fontFamily = FiraSans),
        labelLarge = TextStyle(fontFamily = FiraSansMedium, fontWeight = FontWeight.Medium),
        labelMedium = TextStyle(fontFamily = FiraSansMedium, fontWeight = FontWeight.Medium),
        labelSmall = TextStyle(fontFamily = FiraSansMedium, fontWeight = FontWeight.Medium),
    )

    CompositionLocalProvider(LocalChatReducedMotion provides reducedMotion) {
        MaterialTheme(
            colorScheme = colorScheme,
            typography = typography,
            content = content,
        )
    }
}
