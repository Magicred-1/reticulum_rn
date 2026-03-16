package expo.modules.reticulum

import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.Package

class ReticulumPackage : Package {
    override fun createModules(): List<Module> = listOf(ReticulumModule())
}
